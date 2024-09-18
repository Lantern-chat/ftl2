use std::future::Future;

use http::StatusCode;
use http_body_util::BodyExt as _;

pub use crate::body::Json;

use crate::{body::BodyError, IntoResponse, Request, Response};

use super::{body::body_error_to_response, FromRequest};

#[derive(Debug, thiserror::Error)]
pub enum JsonRejectionError {
    #[error(transparent)]
    Json(#[from] json_impl::Error),

    #[error("An error occurred while reading the body: {0}")]
    BodyError(#[from] BodyError),
}

impl IntoResponse for JsonRejectionError {
    fn into_response(self) -> Response {
        match self {
            // assume badly formatted JSON is a bad request
            JsonRejectionError::Json(_) => StatusCode::BAD_REQUEST.into_response(),
            JsonRejectionError::BodyError(err) => body_error_to_response(err),
        }
    }
}

impl<S, T> FromRequest<S> for Json<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = JsonRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // collect body in non-contiguous memory and then parse it
            let body = req.body_mut().take().collect().await.map_err(JsonRejectionError::BodyError)?;

            #[cfg(not(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64"))))]
            let value = {
                use bytes::Buf;
                // serde-json can read from a non-contiguous buffer using the Reader object
                json_impl::from_reader(body.aggregate().reader())?
            };

            #[cfg(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
            let value = {
                // unknown if sonic-rs can read from a non-contiguous buffer?
                json_impl::from_slice(&body.to_bytes())?
            };

            Ok(Json(value))
        }
    }
}
