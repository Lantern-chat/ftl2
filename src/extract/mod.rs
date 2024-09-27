use core::future::Future;
use std::{convert::Infallible, ops::Deref, str::FromStr as _, sync::Arc};

use http::{uri::Authority, Extensions, HeaderMap, HeaderName, Method, StatusCode, Uri, Version};

use crate::{body::Body, Error, IntoResponse, Request, RequestParts, Response};

pub trait FromRequestParts<S>: Sized + Send + 'static {
    type Rejection: Into<Error> + Send + 'static;

    fn from_request_parts(
        parts: &mut RequestParts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send;
}

mod private {
    #[derive(Debug, Clone, Copy)]
    pub enum ViaParts {}

    #[derive(Debug, Clone, Copy)]
    pub enum ViaRequest {}
}

pub trait FromRequest<S, Z = private::ViaRequest>: Sized + Send + 'static {
    type Rejection: Into<Error> + Send + 'static;

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send;
}

impl<S> FromRequest<S> for Request {
    type Rejection = Infallible;

    fn from_request(req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(req)
    }
}

impl<S> FromRequest<S> for Body {
    type Rejection = Infallible;

    fn from_request(req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(req.into_body())
    }
}

impl<S> FromRequest<S> for () {
    type Rejection = Infallible;

    fn from_request(_req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(())
    }
}

impl<S, T> FromRequest<S, private::ViaParts> for T
where
    S: Send + Sync,
    T: FromRequestParts<S>,
{
    type Rejection = <Self as FromRequestParts<S>>::Rejection;

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let (mut parts, _) = req.into_parts();
            Self::from_request_parts(&mut parts, state).await
        }
    }
}

pub mod body;
pub mod form;
pub mod path;
pub mod query;
pub mod real_ip;
pub mod scheme;

pub use crate::body::Form;

#[cfg(feature = "json")]
mod json;
#[cfg(feature = "json")]
pub use json::Json;

#[cfg(feature = "cbor")]
mod cbor;
#[cfg(feature = "cbor")]
pub use cbor::Cbor;

pub mod one_of;

pub use body::{CollectedBytes, Limited};
pub use path::Path;

macro_rules! impl_from_request {
    ([$($t:ident),*], $last:ident) => {
        impl<S, $($t,)* $last> FromRequestParts<S> for ($($t,)* $last,)
        where
            $($t: FromRequestParts<S> + Send,)*
            $last: FromRequestParts<S> + Send,
            S: Send + Sync,
        {
            type Rejection = Error;

            fn from_request_parts(
                parts: &mut RequestParts,
                state: &S,
            ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
                async move {
                    Ok((
                        $($t::from_request_parts(parts, state).await.map_err(Into::into)?,)*
                        $last::from_request_parts(parts, state).await.map_err(Into::into)?,
                    ))
                }
            }
        }

        impl<$($t,)* $last, S> FromRequest<S> for ($($t,)* $last,)
        where
            $($t: FromRequestParts<S> + Send,)*
            $last: FromRequest<S> + Send,
            S: Send + Sync,
        {
            type Rejection = crate::Error;

            fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
                async move {
                    #[allow(unused_mut)]
                    let (mut parts, body) = req.into_parts();

                    Ok((
                        $($t::from_request_parts(&mut parts, state).await.map_err(Into::into)?,)*
                        $last::from_request(Request::from_parts(parts, body), state).await.map_err(Into::into)?,
                    ))
                }
            }
        }
    };
}

all_the_tuples!(impl_from_request);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct State<S>(pub S);

impl<S> Deref for State<S> {
    type Target = S;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Extension<E>(pub E);

impl<E> Deref for Extension<E> {
    type Target = E;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for State<S>
where
    S: Clone + Send + 'static,
{
    type Rejection = Infallible;

    fn from_request_parts(
        _parts: &mut RequestParts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        core::future::ready(Ok(State(state.clone())))
    }
}

impl<S, E> FromRequestParts<S> for Extension<E>
where
    E: Clone + Send + Sync + 'static,
{
    type Rejection = Error;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        core::future::ready(match parts.extensions.get::<E>() {
            Some(extension) => Ok(Extension(extension.clone())),
            None => Err(Error::MissingExtension),
        })
    }
}

impl<S> FromRequestParts<S> for () {
    type Rejection = Infallible;

    fn from_request_parts(
        _parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(())
    }
}

impl<S, T> FromRequest<S> for Option<T>
where
    T: FromRequest<S> + Send,
    S: Send + Sync,
{
    type Rejection = Infallible;

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(T::from_request(req, state).await.ok()) }
    }
}

impl<S, T> FromRequestParts<S> for Option<T>
where
    T: FromRequestParts<S> + Send,
    S: Send + Sync,
{
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(T::from_request_parts(parts, state).await.ok()) }
    }
}

impl<S, T> FromRequest<S> for Result<T, T::Rejection>
where
    T: FromRequest<S, Rejection: Send + 'static>,
    S: Send + Sync + 'static,
{
    type Rejection = Infallible;

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(T::from_request(req, state).await) }
    }
}

impl<S, T> FromRequestParts<S> for Result<T, T::Rejection>
where
    T: FromRequestParts<S, Rejection: Send + 'static>,
    S: Send + Sync + 'static,
{
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(T::from_request_parts(parts, state).await) }
    }
}

impl<S> FromRequestParts<S> for Method {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(parts.method.clone())
    }
}

impl<S> FromRequestParts<S> for RequestParts {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(parts.clone())
    }
}

impl<S> FromRequestParts<S> for Uri {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(parts.uri.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("missing authority")]
    MissingAuthority,
    #[error("invalid authority")]
    InvalidAuthority,
}

impl IntoResponse for AuthorityError {
    fn into_response(self) -> Response {
        IntoResponse::into_response(match self {
            AuthorityError::MissingAuthority => ("missing authority", StatusCode::BAD_REQUEST),
            AuthorityError::InvalidAuthority => ("invalid authority", StatusCode::BAD_REQUEST),
        })
    }
}

pub(crate) fn extract_authority(parts: &RequestParts) -> Result<Authority, AuthorityError> {
    let from_uri = parts.uri.authority();

    let from_header =
        parts.headers.get(HeaderName::from_static("host")).ok_or(AuthorityError::MissingAuthority).and_then(
            |hdr| {
                Authority::from_str(hdr.to_str().map_err(|_| AuthorityError::InvalidAuthority)?)
                    .map_err(|_| AuthorityError::InvalidAuthority)
            },
        );

    match (from_uri, from_header) {
        (Some(_), Ok(b)) => Ok(b),          // defer to HOST as what the client intended
        (Some(a), Err(_)) => Ok(a.clone()), // HOST is invalid, but URI is valid
        (None, Ok(b)) => Ok(b),
        (None, Err(e)) => Err(e),
    }
}

impl<S> FromRequestParts<S> for Authority {
    type Rejection = AuthorityError;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ready(extract_authority(parts))
    }
}

impl<S> FromRequestParts<S> for Version {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(parts.version)
    }
}

impl<S> FromRequestParts<S> for HeaderMap {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(parts.headers.clone())
    }
}

impl<S> FromRequestParts<S> for Extensions {
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ok(parts.extensions.clone())
    }
}

#[derive(Clone, Debug)]
pub struct MatchedPath(pub Arc<str>);

impl Deref for MatchedPath {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for MatchedPath {
    type Rejection = Error;

    fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        futures::future::ready(parts.extensions.get::<Self>().cloned().ok_or(Error::MissingMatchedPath))
    }
}
