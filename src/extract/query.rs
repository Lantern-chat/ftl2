use crate::{form_impl, Error, RequestParts};

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

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = Error;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl core::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        core::future::ready(match parts.uri.query().map(form_impl::from_str) {
            Some(Ok(value)) => Ok(Query(value)),
            Some(Err(e)) => Err(e.into()),
            None => Err(Error::MissingQuery),
        })
    }
}
