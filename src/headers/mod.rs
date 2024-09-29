use std::{future::Future, sync::LazyLock};

use headers::{ContentType, Header as HeaderType, HeaderMapExt};

use crate::{extract::FromRequestParts, response::IntoResponseParts, Error, RequestParts, ResponseParts};

pub mod accept_encoding;
pub mod entity_tag;
pub mod server_timing;

pub static APPLICATION_CBOR: LazyLock<ContentType> =
    LazyLock::new(|| ContentType::from("application/cbor".parse::<mime::Mime>().unwrap()));

/// A typed header, which can be extracted from a request and inserted into a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Header<H: HeaderType>(pub H);

impl<S, H> FromRequestParts<S> for Header<H>
where
    H: HeaderType + Send + 'static,
{
    type Rejection = Error;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            match parts.headers.typed_try_get::<H>() {
                Ok(Some(header)) => Ok(Header(header)),
                Ok(None) => Err(Error::MissingHeader(H::name().as_str())),
                Err(err) => Err(Error::InvalidHeader(H::name().as_str(), err)),
            }
        }
    }
}

impl<H> IntoResponseParts for Header<H>
where
    H: HeaderType,
{
    #[inline]
    fn into_response_parts(self, parts: &mut ResponseParts) {
        parts.headers.typed_insert(self.0);
    }
}
