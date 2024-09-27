use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use super::BodyError;

/// Wrap any `Body` with a `Bytes` data type so long as
/// the error type can be converted to `BodyError`.
///
/// This is especially useful for when you have a `Body` that returns
/// an IO error, but you want to convert it to a `BodyError`.
#[pin_project::pin_project]
pub struct WrappedBody<B> {
    #[pin]
    pub body: B,
}

impl<B> Body for WrappedBody<B>
where
    B: Body<Data = Bytes, Error: Into<BodyError>>,
{
    type Data = Bytes;
    type Error = BodyError;

    #[inline(always)]
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    #[inline(always)]
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.project().body.poll_frame(cx).map(|opt| opt.map(|res| res.map_err(Into::into)))
    }

    #[inline(always)]
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
