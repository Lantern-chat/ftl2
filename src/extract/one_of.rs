use std::future::Future;

use futures::FutureExt;
use http::{HeaderValue, Method, StatusCode};

use crate::{IntoResponse, Request, Response};

use super::FromRequest;

/// Allows for extracting one of multiple potential encodings from a request body,
/// such as JSON, CBOR, or x-www-form-urlencoded. Unlike direct extraction,
/// this will also check the request for
pub struct OneOf<T, P: ExtractOneOf<T>>(pub <P as ExtractOneOf<T>>::Storage);

pub trait Extractable<T>: FromRequest<()> {
    fn matches_content_type(content_type: &HeaderValue) -> bool;
    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send;
}

pub trait ExtractOneOf<T>: Send + 'static {
    type Storage: Send + 'static;

    fn extract(
        req: Request,
        content_type: HeaderValue,
    ) -> impl Future<Output = Result<Self::Storage, Response>> + Send;
}

macro_rules! impl_extract_any_tuple {
    ($( $ty:ident ),*) => {
        impl<T, $($ty,)*> ExtractOneOf<T> for ($($ty,)+)
        where
            T: Send + 'static,
            $($ty: Extractable<T>),+
        {
            type Storage = T;

            fn extract(req: Request, content_type: HeaderValue) -> impl Future<Output = Result<Self::Storage, Response>> + Send {
                async move {
                    $(
                        if $ty::matches_content_type(&content_type) {
                            return $ty::extract(req).await;
                        }
                    )*

                    Err(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response())
                }
            }
        }
    };
}

all_the_tuples_no_last_special_case!(impl_extract_any_tuple);

#[derive(thiserror::Error)]
pub enum OneOfRejectionError {
    #[error("The request method is not allowed")]
    MethodNotAllowed,

    #[error("The request is missing required headers")]
    MissingHeaders,

    #[error("The request is missing a content type header")]
    MissingContentType,

    #[error("The request body is not valid")]
    Response(Response),
}

use core::fmt;

impl fmt::Debug for OneOfRejectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl IntoResponse for OneOfRejectionError {
    fn into_response(self) -> Response {
        match self {
            OneOfRejectionError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            OneOfRejectionError::MissingHeaders => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
            OneOfRejectionError::MissingContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response(),
            OneOfRejectionError::Response(res) => res,
        }
    }
}

impl<S, T, P: ExtractOneOf<T>> FromRequest<S> for OneOf<T, P>
where
    T: Send + 'static,
{
    type Rejection = OneOfRejectionError;

    fn from_request(req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // https://stackoverflow.com/a/16339271
        async move {
            if matches!(*req.method(), Method::TRACE) {
                return Err(OneOfRejectionError::MethodNotAllowed);
            }

            if !(req.headers().contains_key(http::header::CONTENT_LENGTH)
                || req.headers().contains_key(http::header::TRANSFER_ENCODING))
            {
                return Err(OneOfRejectionError::MissingHeaders);
            }

            // HeaderValue is cheaper to clone than to lookup for each attempted type,
            // since it uses `Bytes` internally, which is reference-counted.
            let Some(content_type) = req.headers().get(http::header::CONTENT_TYPE).cloned() else {
                return Err(OneOfRejectionError::MissingContentType);
            };

            match P::extract(req, content_type).await {
                Ok(value) => Ok(OneOf(value)),
                Err(res) => Err(OneOfRejectionError::Response(res)),
            }
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

    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send {
        Form::<T>::from_request(req, &()).map(|res| match res {
            Ok(form) => Ok(form.0),
            Err(err) => Err(err.into_response()),
        })
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

    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send {
        Json::<T>::from_request(req, &()).map(|res| match res {
            Ok(json) => Ok(json.0),
            Err(err) => Err(err.into_response()),
        })
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

    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send {
        Cbor::<T>::from_request(req, &()).map(|res| match res {
            Ok(cbor) => Ok(cbor.0),
            Err(err) => Err(err.into_response()),
        })
    }
}

impl<T> Extractable<T> for () {
    fn matches_content_type(_: &HeaderValue) -> bool {
        false
    }

    fn extract(_: Request) -> impl Future<Output = Result<T, Response>> + Send {
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
