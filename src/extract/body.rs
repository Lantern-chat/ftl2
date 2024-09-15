use std::{borrow::Cow, convert::Infallible, future::Future};

use bytes::{Buf, Bytes, BytesMut};
use http::StatusCode;
use http_body_util::{BodyExt, BodyStream, Collected};
use std::future::ready;

use crate::{
    body::{Body, BodyError},
    response::IntoResponse,
    FromRequest, Request, Response,
};

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
    type Rejection = BodyRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { req.body_mut().take().collect().await.map_err(BodyRejectionError::from) }
    }
}

impl<S> FromRequest<S> for Bytes {
    type Rejection = BodyRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            match req.body_mut().take().collect().await {
                Ok(collected) => Ok(collected.to_bytes()),
                Err(e) => Err(BodyRejectionError::from(e)),
            }
        }
    }
}

impl<S> FromRequest<S> for BytesMut {
    type Rejection = BodyRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let collected = req.body_mut().take().collect().await.map_err(BodyRejectionError::from)?;

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
    type Rejection = BodyRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(vec_from_collected(req.body_mut().take().collect().await?)) }
    }
}

impl<S> FromRequest<S> for String {
    type Rejection = StringRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            Ok(String::from_utf8(vec_from_collected(
                req.body_mut().take().collect().await?,
            ))?)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BodyRejectionError {
    #[error("An error occurred while reading the body: {0}")]
    BodyError(#[from] BodyError),
}

#[derive(Debug, thiserror::Error)]
pub enum StringRejectionError {
    #[error("An error occurred while reading the body: {0}")]
    BodyError(#[from] BodyError),

    #[error(transparent)]
    FromUtf8Error(#[from] std::string::FromUtf8Error),
}

fn body_error_to_response(err: BodyError) -> Response {
    IntoResponse::into_response(match err {
        BodyError::Generic(e) => (
            format!("An error occurred while reading the body: {e}").into(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        BodyError::Io(e) => (
            format!("An error occurred while reading the body: {e}").into(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),

        BodyError::StreamAborted => (
            Cow::Borrowed("The body stream was aborted"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        BodyError::LengthLimitError(e) => (
            format!("The body was too large: {e}").into(),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        BodyError::HyperError(err) => match err {
            _ if err.is_parse_too_large() => {
                (Cow::Borrowed("The body was too large"), StatusCode::PAYLOAD_TOO_LARGE)
            }
            _ if err.is_body_write_aborted()
                || err.is_canceled()
                || err.is_closed()
                || err.is_incomplete_message() =>
            {
                (
                    Cow::Borrowed("The request was aborted"),
                    StatusCode::UNPROCESSABLE_ENTITY,
                )
            }
            _ if err.is_timeout() => (Cow::Borrowed("The request timed out"), StatusCode::GATEWAY_TIMEOUT),
            _ if err.is_parse() || err.is_parse_status() => (
                Cow::Borrowed("An error occurred while parsing the body"),
                StatusCode::BAD_REQUEST,
            ),
            _ => (
                Cow::Borrowed("An error occurred while reading the body"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        },
    })
}

impl IntoResponse for BodyRejectionError {
    fn into_response(self) -> Response {
        match self {
            BodyRejectionError::BodyError(err) => body_error_to_response(err),
        }
    }
}

impl IntoResponse for StringRejectionError {
    fn into_response(self) -> Response {
        match self {
            StringRejectionError::BodyError(err) => body_error_to_response(err),
            StringRejectionError::FromUtf8Error(_) => {
                IntoResponse::into_response(("The body was not valid UTF-8", StatusCode::BAD_REQUEST))
            }
        }
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
    type Rejection = BodyRejectionError;

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

#[derive(Debug, thiserror::Error)]
pub enum LimitedRejectionError<B> {
    #[error("The body was too large")]
    TooLarge,

    #[error(transparent)]
    BodyError(#[from] B),
}

impl<B> IntoResponse for LimitedRejectionError<B>
where
    B: IntoResponse,
{
    fn into_response(self) -> Response {
        IntoResponse::into_response(match self {
            LimitedRejectionError::TooLarge => ("The body was too large", StatusCode::PAYLOAD_TOO_LARGE),
            LimitedRejectionError::BodyError(err) => return err.into_response(),
        })
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
    type Rejection = LimitedRejectionError<B::Rejection>;

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        use http_body::Body;

        async move {
            // TODO: Add an extension to override the const limit
            // TODO: Also insert an extension here to check if the
            //          body is too large during collection above
            let limit = N as u64;

            if req.body().size_hint().upper() > Some(limit) || req.body().size_hint().lower() > limit {
                Err(LimitedRejectionError::TooLarge)
            } else {
                Ok(Limited(B::from_request(req, state).await?))
            }
        }
    }
}
