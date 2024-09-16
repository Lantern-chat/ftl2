use crate::{body::BodyError, IntoResponse, Response};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use headers::ContentType;
use http::StatusCode;
use hyper::body::Frame;

use super::Body;

#[must_use]
#[derive(Clone)]
pub struct Json {
    inner: Result<Bytes, ()>,
}

impl Json {
    pub fn new<T: serde::Serialize>(value: T) -> Json {
        match Self::try_new(value) {
            Ok(resp) => resp,
            Err(e) => {
                log::error!("JSON Reply error: {e}");
                Json { inner: Err(()) }
            }
        }
    }

    pub fn try_new<T: serde::Serialize>(value: T) -> Result<Json, json_impl::Error> {
        Ok(Json {
            inner: match json_impl::to_vec(&value) {
                Ok(v) => Ok(Bytes::from(v)),
                Err(e) => return Err(e),
            },
        })
    }

    /// Stream a JSON array. This is useful for streaming large JSON arrays
    /// without needing to hold the entire array in memory. If an error occurs
    /// while encoding the JSON, the array will be truncated at the last
    /// successful element and the error logged.
    #[inline]
    #[must_use]
    pub fn stream_array<S, T, E>(stream: S) -> impl IntoResponse
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        T: serde::Serialize + Send + Sync + 'static,
        E: std::error::Error,
    {
        stream_array(stream)
    }

    /// Like `stream_array`, but for streams that yield `T` instead of results.
    pub fn stream_simple_array<S, T>(stream: S) -> impl IntoResponse
    where
        S: Stream<Item = T> + Send + 'static,
        T: serde::Serialize + Send + Sync + 'static,
    {
        stream_array(stream.map(Result::<_, Infallible>::Ok))
    }

    /// Stream a JSON map. This is useful for streaming large JSON maps
    /// without needing to hold the entire map in memory. If an error occurs
    /// while encoding the JSON, the map will be truncated at the last
    /// successful element and the error logged.
    #[inline]
    #[must_use]
    pub fn stream_map<S, K, T, E>(stream: S) -> impl IntoResponse
    where
        S: Stream<Item = Result<(K, T), E>> + Send + 'static,
        K: Borrow<str>,
        T: serde::Serialize + Send + Sync + 'static,
        E: std::error::Error,
    {
        stream_map(stream)
    }

    /// Like `stream_map`, but for streams that yield `(String, T)` pairs
    /// instead of results.
    pub fn stream_simple_map<S, K, T>(stream: S) -> impl IntoResponse
    where
        S: Stream<Item = (K, T)> + Send + 'static,
        K: Borrow<str> + 'static,
        T: serde::Serialize + Send + Sync + 'static,
    {
        stream_map(stream.map(Result::<_, Infallible>::Ok))
    }
}

impl IntoResponse for Json {
    fn into_response(self) -> Response {
        match self.inner {
            Ok(body) => Response::new(Body::from(body)).with_header(ContentType::json()).into_response(),

            Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

use std::{
    borrow::Borrow,
    convert::Infallible,
    mem,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Clone, Copy)]
enum State {
    New,
    First,
    Running,
    Done,
}

#[pin_project::pin_project]
struct JsonArrayBody<S> {
    state: State,

    buffer: Vec<u8>,

    #[pin]
    stream: S,
}

#[pin_project::pin_project]
struct JsonMapBody<S> {
    state: State,

    buffer: String,

    #[pin]
    stream: S,
}

#[allow(clippy::single_char_add_str)] // faster than push(char)
fn stream_map<S, K, T, E>(stream: S) -> impl IntoResponse
where
    S: Stream<Item = Result<(K, T), E>> + Send + 'static,
    K: Borrow<str>,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error,
{
    return Body::wrap(JsonMapBody {
        state: State::New,
        buffer: String::new(),
        stream,
    })
    .with_header(ContentType::json());

    impl<S, K, T, E> hyper::body::Body for JsonMapBody<S>
    where
        S: Stream<Item = Result<(K, T), E>> + Send + 'static,
        K: Borrow<str>,
        T: serde::Serialize + Send + Sync + 'static,
        E: std::error::Error,
    {
        type Data = Bytes;
        type Error = BodyError;

        fn poll_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let mut this = self.project();

            match this.state {
                State::New => {
                    this.buffer.reserve(128);
                    this.buffer.push_str("{");
                    *this.state = State::First
                }
                State::Done => return Poll::Ready(None),
                _ => {}
            }

            while let Some(item) = futures::ready!(this.stream.as_mut().poll_next(cx)) {
                let (key, value) = match item {
                    Ok(item) => item,
                    Err(e) => {
                        log::error!("Error sending JSON map stream: {e}");
                        break;
                    }
                };

                let pos = this.buffer.len();
                let key = key.borrow();

                // most keys will be well-behaved and not need escaping, so `,"key":`
                // extra byte won't hurt anything when the value is serialized
                this.buffer.reserve(key.len() + 4);

                if let State::First = *this.state {
                    this.buffer.push_str("\"");
                    *this.state = State::Running;
                } else {
                    this.buffer.push_str(",\"");
                }

                use std::fmt::Write;
                write!(this.buffer, "{}", v_jsonescape::escape(key)).expect("Unable to write to buffer");

                this.buffer.push_str("\":");

                if let Err(e) = json_impl::to_writer(unsafe { this.buffer.as_mut_vec() }, &value) {
                    this.buffer.truncate(pos); // revert back to previous element
                    log::error!("Error encoding JSON map stream: {e}");
                    break;
                }

                if this.buffer.len() >= (1024 * 8) {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))));
                }
            }

            this.buffer.push_str("}");
            *this.state = State::Done;

            Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))))
        }
    }
}

fn stream_array<S, T, E>(stream: S) -> impl IntoResponse
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error,
{
    return Body::wrap(JsonArrayBody {
        state: State::New,
        buffer: Vec::new(),
        stream,
    })
    .with_header(ContentType::json());

    impl<S, T, E> hyper::body::Body for JsonArrayBody<S>
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        T: serde::Serialize + Send + Sync + 'static,
        E: std::error::Error,
    {
        type Data = Bytes;
        type Error = BodyError;

        fn poll_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let mut this = self.project();

            match this.state {
                State::New => {
                    this.buffer.reserve(128);
                    this.buffer.push(b'[');
                    *this.state = State::First;
                }
                State::Done => return Poll::Ready(None),
                _ => {}
            }

            while let Some(item) = futures::ready!(this.stream.as_mut().poll_next(cx)) {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => {
                        log::error!("Error sending JSON array stream: {e}");
                        break;
                    }
                };

                let pos = this.buffer.len();

                if let State::First = *this.state {
                    *this.state = State::Running;
                } else {
                    this.buffer.push(b',');
                }

                if let Err(e) = json_impl::to_writer(&mut this.buffer, &item) {
                    this.buffer.truncate(pos); // revert back to previous element
                    log::error!("Error encoding JSON array stream: {e}");
                    break;
                }

                if this.buffer.len() >= (1024 * 8) {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))));
                }
            }

            this.buffer.push(b']');
            *this.state = State::Done;

            Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))))
        }
    }
}
