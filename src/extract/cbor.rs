use std::future::Future;

use http::StatusCode;
use http_body_util::BodyExt as _;

pub use crate::body::Cbor;

use crate::{body::BodyError, IntoResponse, Request, Response};

use super::{body::body_error_to_response, FromRequest};

#[derive(Debug, thiserror::Error)]
pub enum CborRejectionError {
    #[error(transparent)]
    Cbor(#[from] ciborium::de::Error<std::io::Error>),

    #[error("An error occurred while reading the body: {0}")]
    BodyError(#[from] BodyError),
}

impl<S, T> FromRequest<S> for Cbor<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = CborRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // collect body in non-contiguous memory and then parse it
            let body = req.body_mut().take().collect().await.map_err(CborRejectionError::BodyError)?;

            let value = {
                use bytes::Buf;

                // ciborium can read from a non-contiguous buffer using the Reader object
                ciborium::de::from_reader(body.aggregate().reader())?
            };

            Ok(Cbor(value))
        }
    }
}

impl IntoResponse for CborRejectionError {
    fn into_response(self) -> Response {
        match self {
            // assume badly formatted JSON is a bad request
            CborRejectionError::Cbor(_) => StatusCode::BAD_REQUEST.into_response(),
            CborRejectionError::BodyError(err) => body_error_to_response(err),
        }
    }
}
