use crate::{IntoResponse, RequestParts, Response};

use super::FromRequestParts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Query<T>(pub T);

impl<T> core::ops::Deref for Query<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FromQueryError {
    #[error(transparent)]
    De(#[from] serde_urlencoded::de::Error),

    #[error("The query is missing")]
    MissingQuery,
}

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = FromQueryError;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl core::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        core::future::ready(match parts.uri.query().map(serde_urlencoded::from_str) {
            Some(Ok(value)) => Ok(Query(value)),
            Some(Err(e)) => Err(e.into()),
            None => Err(FromQueryError::MissingQuery),
        })
    }
}

impl IntoResponse for FromQueryError {
    fn into_response(self) -> Response {
        match self {
            FromQueryError::De(e) => (e.to_string(), http::StatusCode::BAD_REQUEST).into_response(),
            FromQueryError::MissingQuery => http::StatusCode::BAD_REQUEST.into_response(),
        }
    }
}
