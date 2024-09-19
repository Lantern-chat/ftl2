use std::{
    any::TypeId,
    error::Error,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, Incoming};
use tokio::sync::mpsc;

use http_body_util::{Full, StreamBody};
use tokio_stream::wrappers::ReceiverStream;

#[cfg(feature = "json")]
mod json;
#[cfg(feature = "json")]
pub use json::Json;

#[cfg(feature = "cbor")]
mod cbor;
#[cfg(feature = "cbor")]
pub use cbor::Cbor;

mod form;
pub use form::Form;

pub mod disposition;
pub use disposition::Disposition;

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("Hyper error: {0}")]
    HyperError(#[from] hyper::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Stream Aborted")]
    StreamAborted,

    #[error(transparent)]
    LengthLimitError(#[from] http_body_util::LengthLimitError),

    #[error(transparent)]
    Generic(Box<dyn Error + Send + Sync + 'static>),
}

#[derive(Default)]
#[repr(transparent)]
#[must_use]
pub struct Body(BodyInner);

#[derive(Default)]
#[pin_project::pin_project(project = BodyProj)]
enum BodyInner {
    #[default]
    Empty,
    Incoming(#[pin] hyper::body::Incoming),
    Full(#[pin] Full<Bytes>),
    Channel(#[pin] StreamBody<ReceiverStream<Result<Frame<Bytes>, BodyError>>>),
    Stream(#[pin] StreamBody<futures::stream::BoxStream<'static, Result<Frame<Bytes>, BodyError>>>),
    //Buf(#[pin] Full<Pin<Box<dyn Buf + Send + 'static>>>),
    Dyn(#[pin] Pin<Box<dyn HttpBody<Data = Bytes, Error = BodyError> + Send + 'static>>),
}

// assert Send
const _: () = {
    const fn test_send<T: Send>() {}
    test_send::<Body>();
};

impl HttpBody for Body {
    type Data = Bytes;
    type Error = BodyError;

    #[inline]
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.get_mut().0).poll_frame(cx)
    }

    #[inline]
    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    #[inline]
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.0.size_hint()
    }
}

impl HttpBody for BodyInner {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            BodyProj::Empty => Poll::Ready(None),
            BodyProj::Incoming(incoming) => incoming.poll_frame(cx).map_err(BodyError::from),
            BodyProj::Full(full) => full.poll_frame(cx).map_err(|_| unreachable!()),
            //BodyProj::Buf(buf) => buf.poll_frame(cx).map_err(|_| unreachable!()),
            BodyProj::Channel(stream) => stream.poll_frame(cx),
            BodyProj::Stream(stream) => stream.poll_frame(cx),
            BodyProj::Dyn(body) => body.poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Incoming(inner) => inner.is_end_stream(),
            Self::Full(inner) => inner.is_end_stream(),
            Self::Channel(inner) => inner.is_end_stream(),
            Self::Stream(inner) => inner.is_end_stream(),
            Self::Dyn(inner) => inner.is_end_stream(),
        }
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self {
            Self::Empty => hyper::body::SizeHint::new(),
            Self::Incoming(inner) => inner.size_hint(),
            Self::Full(inner) => inner.size_hint(),
            Self::Channel(inner) => inner.size_hint(),
            Self::Stream(inner) => inner.size_hint(),
            Self::Dyn(inner) => inner.size_hint(),
        }
    }
}

impl From<()> for Body {
    #[inline]
    fn from(_: ()) -> Self {
        Body::empty()
    }
}

impl From<Bytes> for Body {
    #[inline]
    fn from(value: Bytes) -> Self {
        Body(BodyInner::Full(Full::new(value)))
    }
}

impl From<Full<Bytes>> for Body {
    #[inline]
    fn from(value: Full<Bytes>) -> Self {
        Body(BodyInner::Full(value))
    }
}

impl From<Vec<u8>> for Body {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Bytes::from(value).into()
    }
}

impl From<String> for Body {
    #[inline]
    fn from(value: String) -> Self {
        Bytes::from(value).into()
    }
}

impl From<Incoming> for Body {
    #[inline]
    fn from(incoming: Incoming) -> Self {
        Body(BodyInner::Incoming(incoming))
    }
}

impl Body {
    /// Create a new empty body that yields no frames.
    pub const fn empty() -> Body {
        Body(BodyInner::Empty)
    }

    /// Takes the body, leaving [`Body::empty()`] in its place.
    pub fn take(&mut self) -> Self {
        std::mem::replace(self, Body::empty())
    }

    /// Takes the body if empty, leaving `Body::empty()` in its place. Returns `None` if it was previously empty.
    pub fn take_nonempty(&mut self) -> Option<Self> {
        if matches!(self.0, BodyInner::Empty) {
            None
        } else {
            Some(self.take())
        }
    }

    /// Create a new bounded channel with the given capacity where
    /// the receiver will forward given frames to the HTTP Body.
    pub fn channel(capacity: usize) -> (Self, BodySender) {
        let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, BodyError>>(capacity);

        (
            Body(BodyInner::Channel(StreamBody::new(ReceiverStream::new(rx)))),
            BodySender(tx),
        )
    }

    /// Creates an HTTP Body by wrapping a Stream of byte frames.
    pub fn stream<S>(stream: S) -> Body
    where
        S: futures::Stream<Item = Result<Frame<Bytes>, BodyError>> + Send + 'static,
    {
        Body(BodyInner::Stream(StreamBody::new(Box::pin(stream))))
    }

    pub fn wrap<B>(body: B) -> Body
    where
        B: HttpBody<Data = Bytes, Error = BodyError> + Send + 'static,
    {
        Body(BodyInner::Dyn(Box::pin(body)))
    }
}

pub struct BodySender(mpsc::Sender<Result<Frame<Bytes>, BodyError>>);

impl std::ops::Deref for BodySender {
    type Target = mpsc::Sender<Result<Frame<Bytes>, BodyError>>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BodySender {
    /// Aborts the body stream with an [`BodyError::StreamAborted`] error
    pub async fn abort(self) -> bool {
        self.send(Err(BodyError::StreamAborted)).await.is_ok()
    }
}

impl Body {
    pub(crate) fn from_any_body<B>(body: B) -> Self
    where
        B: http_body::Body<Data = Bytes, Error: Error + Send + Sync + 'static> + Send + 'static,
    {
        use core::mem::ManuallyDrop;

        let body = ManuallyDrop::new(body);
        let ptr = &body as *const ManuallyDrop<B>;

        match TypeId::of::<B>() {
            id if id == TypeId::of::<hyper::body::Incoming>() => {
                // SAFETY: we know the type is hyper::body::Incoming
                Body::from(unsafe { std::ptr::read::<hyper::body::Incoming>(ptr.cast()) })
            }
            id if id == TypeId::of::<Body>() => {
                // SAFETY: we know the type is Body
                unsafe { std::ptr::read::<Body>(ptr.cast()) }
            }
            id if id == TypeId::of::<Full<Bytes>>() => {
                // SAFETY: we know the type is Full<Bytes>
                Body(BodyInner::Full(unsafe { std::ptr::read(ptr.cast()) }))
            }
            _ => {
                use http_body_util::BodyExt;

                Body::wrap(ManuallyDrop::into_inner(body).map_err(|e| BodyError::Generic(Box::new(e))))
            }
        }
    }
}
