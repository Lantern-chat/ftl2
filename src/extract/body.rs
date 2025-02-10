use std::{borrow::Cow, convert::Infallible, future::Future};

use bytes::{Buf, Bytes, BytesMut};
use http_body_util::{BodyExt, BodyStream, Collected};
use std::future::ready;

use crate::{body::Body, FromRequest, Request};

impl<S> FromRequest<S> for BodyStream<Body> {
    type Rejection = Infallible;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        ready(Ok(BodyStream::new(req.body_mut().take())))
    }
}

/// Aggregated body of a request, not necessary in contiguous memory.
///
/// Notably, using the [`.aggregate()`](Collected::aggregate) method this can be used as a Reader,
/// without requiring the entire body be in contiguous memory, allowing for
/// more efficient handling of large/slow bodies.
pub type CollectedBytes = Collected<Bytes>;

impl<S> FromRequest<S> for CollectedBytes {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(req.body_mut().take().collect().await?) }
    }
}

impl<S> FromRequest<S> for Bytes {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(req.body_mut().take().collect().await?.to_bytes()) }
    }
}

#[cfg(feature = "ws")]
impl<S> FromRequest<S> for tokio_tungstenite::tungstenite::Utf8Bytes {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            tokio_tungstenite::tungstenite::Utf8Bytes::try_from(req.body_mut().take().collect().await?.to_bytes())
                .map_err(crate::Error::from)
        }
    }
}

impl<S> FromRequest<S> for BytesMut {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let collected = req.body_mut().take().collect().await?;

            let buf = collected.aggregate();

            use bytes::BufMut;

            let mut bytes = BytesMut::with_capacity(buf.remaining());
            bytes.put(buf);

            Ok(bytes)
        }
    }
}

// avoid going through `.to_bytes().to_vec()`, which may allocate twice
fn vec_from_collected(collected: Collected<Bytes>) -> Vec<u8> {
    let mut buf = collected.aggregate();
    let mut vec = vec![0u8; buf.remaining()];

    buf.copy_to_slice(&mut vec);

    vec
}

impl<S> FromRequest<S> for Vec<u8> {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(vec_from_collected(req.body_mut().take().collect().await?)) }
    }
}

impl Body {
    /// Attempt to convert the body into a [`String`]
    pub async fn to_string(&mut self) -> Result<String, crate::Error> {
        Ok(String::from_utf8(vec_from_collected(self.take().collect().await?)).map_err(|e| e.utf8_error())?)
    }
}

impl<S> FromRequest<S> for String {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { req.body_mut().to_string().await }
    }
}

impl<S> FromRequest<S> for Cow<'static, str> {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(Cow::Owned(req.body_mut().to_string().await?)) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct LossyString(pub String);

impl core::ops::Deref for LossyString {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequest<S> for LossyString {
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let vec = vec_from_collected(req.body_mut().take().collect().await?);

            Ok(LossyString(match String::from_utf8_lossy(&vec) {
                Cow::Borrowed(_) => unsafe { String::from_utf8_unchecked(vec) },
                Cow::Owned(s) => s,
            }))
        }
    }
}

pub trait LimitedBody<const N: usize> {
    type Body;
}

pub struct Limited<const N: usize, B: LimitedBody<N>>(pub <B as LimitedBody<N>>::Body);

impl<S, const N: usize, B> FromRequest<S> for Limited<N, B>
where
    B: LimitedBody<N, Body = B> + FromRequest<S>,
    S: Send + Sync,
{
    type Rejection = crate::Error;

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        use http_body::Body;

        async move {
            // TODO: Add an extension to override the const limit
            // TODO: Also insert an extension here to check if the
            //          body is too large during collection above
            let limit = N as u64;

            if req.body().size_hint().upper() > Some(limit) || req.body().size_hint().lower() > limit {
                Err(crate::Error::PayloadTooLarge)
            } else {
                Ok(Limited(B::from_request(req, state).await.map_err(Into::into)?))
            }
        }
    }
}
