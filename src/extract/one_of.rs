use std::future::Future;

use futures::FutureExt;
use http::{HeaderName, HeaderValue, Method, StatusCode};

use crate::{IntoResponse, Request, Response};

use super::FromRequest;

pub struct OneOf<T, P: ExtractOneOf<T>>(pub <P as ExtractOneOf<T>>::Storage);

pub trait Extractable<T>: FromRequest<()> {
    fn matches_content_type(content_type: &HeaderValue) -> bool;
    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send;
}

pub trait ExtractOneOf<T>: Send + 'static {
    type Storage: Send + 'static;

    fn extract(req: Request) -> impl Future<Output = Result<Self::Storage, Response>> + Send;
}

macro_rules! impl_extract_any_tuple {
    ($( $ty:ident ),*) => {
        impl<T, $($ty,)*> ExtractOneOf<T> for ($($ty,)+)
        where
            T: Send + 'static,
            $($ty: Extractable<T>),+
        {
            type Storage = T;

            fn extract(req: Request) -> impl Future<Output = Result<Self::Storage, Response>> + Send {
                async move {
                    let Some(content_type) = req.headers().get(HeaderName::from_static("content-type")) else {
                        return Err(StatusCode::BAD_REQUEST.into_response());
                    };

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

    #[error("The request body is not valid")]
    Response(Response),
}

use core::fmt;

impl fmt::Debug for OneOfRejectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<S, T, P: ExtractOneOf<T>> FromRequest<S> for OneOf<T, P>
where
    T: Send + 'static,
{
    type Rejection = OneOfRejectionError;

    fn from_request(req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            if !matches!(*req.method(), Method::POST | Method::PUT) {
                return Err(OneOfRejectionError::MethodNotAllowed);
            }

            match P::extract(req).await {
                Ok(value) => Ok(OneOf(value)),
                Err(res) => Err(OneOfRejectionError::Response(res)),
            }
        }
    }
}

impl IntoResponse for OneOfRejectionError {
    fn into_response(self) -> Response {
        match self {
            OneOfRejectionError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            OneOfRejectionError::Response(res) => res,
        }
    }
}

#[cfg(feature = "json")]
impl<T> Extractable<T> for super::Json
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    fn matches_content_type(content_type: &HeaderValue) -> bool {
        content_type == "application/json"
    }

    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send {
        super::Json::<T>::from_request(req, &()).map(|res| match res {
            Ok(json) => Ok(json.0),
            Err(err) => Err(err.into_response()),
        })
    }
}

#[cfg(feature = "cbor")]
impl<T> Extractable<T> for super::Cbor
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    fn matches_content_type(content_type: &HeaderValue) -> bool {
        content_type == "application/cbor"
    }

    fn extract(req: Request) -> impl Future<Output = Result<T, Response>> + Send {
        super::Cbor::<T>::from_request(req, &()).map(|res| match res {
            Ok(cbor) => Ok(cbor.0),
            Err(err) => Err(err.into_response()),
        })
    }
}
