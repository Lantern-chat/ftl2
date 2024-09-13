use std::{
    any::TypeId,
    error::Error,
    mem::ManuallyDrop,
    pin::Pin,
    sync::Once,
    task::{Context, Poll},
};

use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, Incoming};
use tokio::sync::mpsc;

use http_body_util::{Full, StreamBody};
use tokio_stream::wrappers::ReceiverStream;
use tower_layer::Layer;

use crate::{Request, Service};

pub struct ConvertAnyBody<S>(pub S);
pub struct ConvertIncoming<S>(pub S);

impl<S> Layer<S> for ConvertAnyBody<()> {
    type Service = ConvertAnyBody<S>;

    fn layer(&self, service: S) -> Self::Service {
        ConvertAnyBody(service)
    }
}

impl<S> Layer<S> for ConvertIncoming<()> {
    type Service = ConvertIncoming<S>;

    fn layer(&self, service: S) -> Self::Service {
        ConvertIncoming(service)
    }
}

impl<S, B> Service<http::Request<B>> for ConvertAnyBody<S>
where
    S: Service<Request>,
    B: http_body::Body<Data = Bytes, Error: Error + Send + Sync + 'static> + Send + 'static,
{
    type Error = S::Error;
    type Response = S::Response;

    #[cfg(feature = "tower-service")]
    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(
        &self,
        req: http::Request<B>,
    ) -> impl std::future::Future<Output = Result<Self::Response, Self::Error>> + Send + 'static
    {
        use http_body_util::BodyExt;

        let (parts, body) = req.into_parts();

        // we can more efficiently handle hyper::body::Incoming, so do some shinanigans
        let body = if TypeId::of::<B>() == TypeId::of::<hyper::body::Incoming>() {
            static WARNING: Once = Once::new();

            WARNING.call_once(|| log::warn!("Warning: ConvertAnyBody called with hyper::body::Incoming, consider using ConvertIncoming instead"));

            // SAFETY: we know the type is hyper::body::Incoming, this is effectively a transmute
            Body::from(unsafe {
                let body = ManuallyDrop::new(body);
                std::ptr::read::<hyper::body::Incoming>(&body as *const _ as *const _)
            })
        } else {
            Body::wrap(body.map_err(|e| BodyError::Generic(Box::new(e))))
        };

        self.0.call(http::Request::from_parts(parts, body))
    }
}

impl<S> Service<http::Request<hyper::body::Incoming>> for ConvertIncoming<S>
where
    S: Service<Request>,
{
    type Error = S::Error;
    type Response = S::Response;

    #[cfg(feature = "tower-service")]
    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(
        &self,
        req: http::Request<hyper::body::Incoming>,
    ) -> impl std::future::Future<Output = Result<Self::Response, Self::Error>> + Send + 'static
    {
        let (parts, body) = req.into_parts();
        self.0
            .call(http::Request::from_parts(parts, Body::from(body)))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("Hyper error: {0}")]
    HyperError(#[from] hyper::Error),

    #[error("Stream Aborted")]
    StreamAborted,

    #[error(transparent)]
    LengthLimitError(#[from] http_body_util::LengthLimitError),

    #[error(transparent)]
    Generic(Box<dyn Error + Send + Sync + 'static>),
}

#[derive(Default)]
#[repr(transparent)]
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

impl From<Bytes> for Body {
    #[inline]
    fn from(value: Bytes) -> Self {
        Body(BodyInner::Full(Full::new(value)))
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
