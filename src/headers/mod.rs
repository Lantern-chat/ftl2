use std::future::Future;

use crate::{extract::FromRequestParts, response::IntoResponseParts, IntoResponse, Response};

use headers::{Header as HeaderType, HeaderMapExt};
use http::{request::Parts, StatusCode};

pub mod accept_encoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Header<H: HeaderType>(pub H);

pub enum HeaderError {
    HeaderNotFound(&'static str),
    InvalidHeader(&'static str, headers::Error),
}

impl IntoResponse for HeaderError {
    fn into_response(self) -> Response {
        IntoResponse::into_response(match self {
            HeaderError::HeaderNotFound(name) => (format!("Missing Header: {name}"), StatusCode::BAD_REQUEST),
            HeaderError::InvalidHeader(name, err) => {
                (format!("Invalid Header: {name}: {err}"), StatusCode::BAD_REQUEST)
            }
        })
    }
}

impl<S, H> FromRequestParts<S> for Header<H>
where
    H: HeaderType + Send + 'static,
{
    type Rejection = HeaderError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            match parts.headers.typed_try_get::<H>() {
                Ok(Some(header)) => Ok(Header(header)),
                Ok(None) => Err(HeaderError::HeaderNotFound(H::name().as_str())),
                Err(err) => Err(HeaderError::InvalidHeader(H::name().as_str(), err)),
            }
        }
    }
}

impl<H> IntoResponseParts for Header<H>
where
    H: HeaderType,
{
    fn into_response_parts(self, parts: &mut http::response::Parts) {
        parts.headers.typed_insert(self.0);
    }
}
