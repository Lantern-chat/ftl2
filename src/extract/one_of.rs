use core::future::Future;

use futures::TryFutureExt as _;
use http::{HeaderValue, Method};

use crate::{Error, Request};

use super::FromRequest;

/// Allows for extracting one of multiple potential encodings from a request body,
/// such as JSON, CBOR, or x-www-form-urlencoded. Unlike direct extraction,
/// this will also check the request for
pub struct OneOf<T, P: ExtractOneOf<T>>(pub <P as ExtractOneOf<T>>::Storage);

pub trait Extractable<T>: FromRequest<()> {
    fn matches_content_type(content_type: &HeaderValue) -> bool;
    fn extract(req: Request) -> impl Future<Output = Result<T, Error>> + Send;
}

pub trait ExtractOneOf<T>: Send + 'static {
    type Storage: Send + 'static;

    fn extract(
        req: Request,
        content_type: HeaderValue,
    ) -> impl Future<Output = Result<Self::Storage, Error>> + Send;
}

macro_rules! impl_extract_any_tuple {
    ($( $ty:ident ),*) => {
        impl<T, $($ty,)*> ExtractOneOf<T> for ($($ty,)+)
        where
            T: Send + 'static,
            $($ty: Extractable<T>),+
        {
            type Storage = T;

            fn extract(req: Request, content_type: HeaderValue) -> impl Future<Output = Result<Self::Storage, Error>> + Send {
                async move {
                    $(
                        if $ty::matches_content_type(&content_type) {
                            return $ty::extract(req).await;
                        }
                    )*

                    Err(Error::UnsupportedMediaType)
                }
            }
        }
    };
}

all_the_tuples_no_last_special_case!(impl_extract_any_tuple);

impl<S, T, P: ExtractOneOf<T>> FromRequest<S> for OneOf<T, P>
where
    T: Send + 'static,
{
    type Rejection = crate::Error;

    fn from_request(req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // https://stackoverflow.com/a/16339271
        async move {
            if matches!(*req.method(), Method::TRACE) {
                return Err(Error::MethodNotAllowed);
            }

            if !(req.headers().contains_key(http::header::CONTENT_LENGTH)
                || req.headers().contains_key(http::header::TRANSFER_ENCODING))
            {
                return Err(Error::MissingHeader("Content-Length or Transfer-Encoding"));
            }

            // HeaderValue is cheaper to clone than to lookup for each attempted type,
            // since it uses `Bytes` internally, which is reference-counted.
            let Some(content_type) = req.headers().get(http::header::CONTENT_TYPE).cloned() else {
                return Err(Error::MissingHeader("Content-Type"));
            };

            Ok(OneOf(P::extract(req, content_type).await?))
        }
    }
}

use super::Form;

impl<T> Extractable<T> for Form
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    #[inline]
    fn matches_content_type(content_type: &HeaderValue) -> bool {
        content_type == "application/x-www-form-urlencoded"
    }

    fn extract(req: Request) -> impl Future<Output = Result<T, Error>> + Send {
        Form::<T>::from_request(req, &()).map_ok(|res| res.0)
    }
}

#[cfg(feature = "json")]
use super::Json;

#[cfg(feature = "json")]
impl<T> Extractable<T> for Json
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    #[inline]
    fn matches_content_type(content_type: &HeaderValue) -> bool {
        content_type == "application/json"
    }

    fn extract(req: Request) -> impl Future<Output = Result<T, Error>> + Send {
        Json::<T>::from_request(req, &()).map_ok(|res| res.0)
    }
}

#[cfg(feature = "cbor")]
use super::Cbor;

#[cfg(feature = "cbor")]
impl<T> Extractable<T> for Cbor
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    #[inline]
    fn matches_content_type(content_type: &HeaderValue) -> bool {
        content_type == "application/cbor"
    }

    fn extract(req: Request) -> impl Future<Output = Result<T, Error>> + Send {
        Cbor::<T>::from_request(req, &()).map_ok(|res| res.0)
    }
}

impl<T> Extractable<T> for () {
    fn matches_content_type(_: &HeaderValue) -> bool {
        false
    }

    fn extract(_: Request) -> impl Future<Output = Result<T, Error>> + Send {
        async move { unreachable!() }
    }
}

#[cfg(not(feature = "cbor"))]
type Cbor = ();
#[cfg(not(feature = "json"))]
type Json = ();

/// A type that can be extracted from a request body using any format supported.
///
/// Currently this includes JSON, CBOR, and x-www-form-urlencoded.
///
/// If JSON or CBOR support is disabled,
/// this type will reject requests made with those content types.
pub type OneOfAny<T> = OneOf<T, (Json, Cbor, Form)>;
