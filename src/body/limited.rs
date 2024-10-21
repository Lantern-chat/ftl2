use bytes::{Buf, Bytes};
use http_body::{Body, Frame, SizeHint};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

use super::BodyError;

#[pin_project::pin_project]
pub struct Limited {
    pub(super) remaining: usize,
    #[pin]
    pub(super) inner: Box<super::Body>,
}

impl Body for Limited {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();

        match this.inner.poll_frame(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(match frame.data_ref() {
                Some(data) => match this.remaining.checked_sub(data.remaining()) {
                    Some(remaining) => {
                        *this.remaining = remaining;
                        Ok(frame)
                    }
                    None => {
                        *this.remaining = 0;
                        Err(BodyError::LengthLimitError)
                    }
                },
                None => Ok(frame), // trailers
            })),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        match u64::try_from(self.remaining) {
            Ok(n) => {
                let mut hint = self.inner.size_hint();
                if hint.lower() >= n {
                    hint.set_exact(n)
                } else if let Some(max) = hint.upper() {
                    hint.set_upper(n.min(max))
                } else {
                    hint.set_upper(n)
                }
                hint
            }
            Err(_) => self.inner.size_hint(),
        }
    }
}
