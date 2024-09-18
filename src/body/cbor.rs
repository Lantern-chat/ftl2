use crate::{body::BodyError, headers::APPLICATION_CBOR, IntoResponse, Response};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use http::StatusCode;
use hyper::body::Frame;

use super::Body;

#[must_use]
#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct Cbor<T = ()>(pub T);

type Error = ciborium::ser::Error<std::io::Error>;

impl Cbor {
    pub fn try_new<T: serde::Serialize>(value: T) -> Result<Response, Error> {
        let mut buf = Vec::with_capacity(size_of::<T>().max(64));

        match ciborium::ser::into_writer(&value, &mut buf) {
            Ok(()) => Ok(Body::from(buf).with_header(APPLICATION_CBOR.clone()).into_response()),
            Err(e) => Err(e),
        }
    }

    /// Stream an array of objects as individual CBOR-encoded objects, one after another. This is useful for streaming large
    /// arrays without needing to hold the entire array in memory. If an error occurs while encoding an object, the stream will
    /// be truncated at the last successful object and the error logged.
    ///
    /// Note that when decoding the stream, the stream essentially needs to be consumed and deserialized
    /// one at a time until EOF. When using `ciborium` on a read-stream, it will advance the stream
    /// enough such that the next item can be immediately read in as well.
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

    /// Like [`stream_array`](Self::stream_array), but for streams that yield `T` instead of results.
    #[inline]
    #[must_use]
    pub fn stream_simple_array<S, T>(stream: S) -> impl IntoResponse
    where
        S: Stream<Item = T> + Send + 'static,
        T: serde::Serialize + Send + Sync + 'static,
    {
        stream_array(stream.map(Result::<_, Infallible>::Ok))
    }
}

impl<T> IntoResponse for Cbor<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> Response {
        match Cbor::try_new(self.0) {
            Ok(resp) => resp,
            Err(e) => {
                log::error!("Error encoding CBOR: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

#[pin_project::pin_project]
struct CborArrayBody<S> {
    buffer: Vec<u8>,

    #[pin]
    stream: S,
}

use std::{
    convert::Infallible,
    mem,
    pin::Pin,
    task::{Context, Poll},
};

pub fn stream_array<S, T, E>(stream: S) -> impl IntoResponse
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error,
{
    return Body::wrap(CborArrayBody {
        buffer: Vec::new(),
        stream,
    })
    .with_header(APPLICATION_CBOR.clone());

    impl<S, T, E> hyper::body::Body for CborArrayBody<S>
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

            while let Some(item) = futures::ready!(this.stream.as_mut().poll_next(cx)) {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => {
                        log::error!("Error sending CBOR stream: {e}");
                        break;
                    }
                };

                let pos = this.buffer.len();

                if let Err(e) = ciborium::into_writer(&item, &mut this.buffer) {
                    this.buffer.truncate(pos);
                    log::error!("Error encoding CBOR stream: {e}");
                    break;
                }

                if this.buffer.len() >= (1024 * 8) {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))));
                }
            }

            Poll::Ready(match this.buffer.is_empty() {
                false => Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))),
                true => None,
            })
        }
    }
}
