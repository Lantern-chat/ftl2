use bytes::{Bytes, BytesMut};
use http::{
    header::{HeaderMap, HeaderName, HeaderValue},
    response::Parts,
    Extensions, StatusCode,
};
use std::{borrow::Cow, convert::Infallible};

use crate::{body::Body, Response};

pub trait IntoResponseParts {
    fn into_response_parts(self, parts: &mut Parts);
}

pub trait IntoResponse {
    #[must_use]
    fn into_response(self) -> Response;
}

impl IntoResponseParts for () {
    #[inline]
    fn into_response_parts(self, _parts: &mut Parts) {}
}

impl IntoResponseParts for StatusCode {
    #[inline]
    fn into_response_parts(self, parts: &mut Parts) {
        parts.status = self;
    }
}

impl IntoResponseParts for Extensions {
    #[inline]
    fn into_response_parts(self, parts: &mut Parts) {
        parts.extensions.extend(self);
    }
}

impl<T> IntoResponseParts for Option<T>
where
    T: IntoResponseParts,
{
    #[inline]
    fn into_response_parts(self, parts: &mut Parts) {
        if let Some(inner) = self {
            inner.into_response_parts(parts);
        }
    }
}

impl IntoResponseParts for HeaderMap {
    #[inline]
    fn into_response_parts(self, parts: &mut Parts) {
        parts.headers.extend(self);
    }
}

impl<const N: usize> IntoResponseParts for [(HeaderName, HeaderValue); N] {
    #[inline]
    fn into_response_parts(self, parts: &mut Parts) {
        parts.headers.reserve(N);
        for (name, value) in self {
            parts.headers.append(name, value);
        }
    }
}

impl IntoResponse for Response {
    #[inline]
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for () {
    #[inline]
    fn into_response(self) -> Response {
        Response::default()
    }
}

impl IntoResponse for Infallible {
    #[inline]
    fn into_response(self) -> Response {
        match self {}
    }
}

impl IntoResponse for StatusCode {
    #[inline]
    fn into_response(self) -> Response {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = self;
        resp
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: IntoResponse,
{
    #[inline]
    fn into_response(self) -> Response {
        match self {
            Ok(ok) => ok.into_response(),
            Err(err) => err.into_response(),
        }
    }
}

macro_rules! impl_into_response_parts {
    ($($t:ident),*) => {
        impl<$($t,)*> IntoResponseParts for ($($t,)*)
        where
            $($t: IntoResponseParts,)*
        {
            #[allow(non_snake_case)]
            fn into_response_parts(self, parts: &mut Parts) {
                let ($($t,)*) = self;
                $($t.into_response_parts(parts);)*
            }
        }
    };
}

all_the_tuples_no_last_special_case!(impl_into_response_parts);

macro_rules! impl_into_response {
    ($($t:ident),*) => {
        #[allow(non_snake_case)]
        impl<R, $($t,)*> IntoResponse for (R, $($t,)*)
        where
            R: IntoResponse,
            $($t: IntoResponseParts,)*
        {
            fn into_response(self) -> Response {
                let (res, $($t,)*) = self;
                let (mut parts, body) = res.into_response().into_parts();
                $($t.into_response_parts(&mut parts);)*
                Response::from_parts(parts, body)
            }
        }
    };
}

all_the_tuples_no_last_special_case!(impl_into_response);

impl<R> IntoResponse for (R,)
where
    R: IntoResponse,
{
    #[inline]
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}

impl IntoResponse for Bytes {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self.into())
    }
}

impl IntoResponse for BytesMut {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self.freeze().into())
    }
}

impl IntoResponse for Vec<u8> {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self.into())
    }
}

impl IntoResponse for String {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self.into())
    }
}

impl IntoResponse for &'static str {
    #[inline]
    fn into_response(self) -> Response {
        Bytes::from_static(self.as_bytes()).into_response()
    }
}

impl IntoResponse for &'static [u8] {
    #[inline]
    fn into_response(self) -> Response {
        Bytes::from_static(self).into_response()
    }
}

impl IntoResponse for Cow<'static, str> {
    #[inline]
    fn into_response(self) -> Response {
        match self {
            Cow::Borrowed(s) => s.into_response(),
            Cow::Owned(s) => s.into_response(),
        }
    }
}

impl IntoResponse for Cow<'static, [u8]> {
    #[inline]
    fn into_response(self) -> Response {
        match self {
            Cow::Borrowed(s) => s.into_response(),
            Cow::Owned(s) => s.into_response(),
        }
    }
}

impl<T> IntoResponse for Box<T>
where
    T: IntoResponse,
{
    #[inline]
    fn into_response(self) -> Response {
        (*self).into_response()
    }
}

impl IntoResponse for Box<str> {
    #[inline]
    fn into_response(self) -> Response {
        self.into_string().into_response()
    }
}

impl IntoResponse for Box<[u8]> {
    #[inline]
    fn into_response(self) -> Response {
        self.into_vec().into_response()
    }
}

impl IntoResponse for HeaderMap {
    #[inline]
    fn into_response(self) -> Response {
        let mut resp = Response::new(Body::empty());
        resp.headers_mut().extend(self);
        resp
    }
}

impl IntoResponse for Parts {
    #[inline]
    fn into_response(self) -> Response {
        Response::from_parts(self, Body::empty())
    }
}

impl<const N: usize> IntoResponse for [(HeaderName, HeaderValue); N] {
    #[inline]
    fn into_response(self) -> Response {
        let mut resp = Response::new(Body::empty());
        resp.headers_mut().reserve(N);
        for (name, value) in self {
            resp.headers_mut().append(name, value.clone());
        }
        resp
    }
}

impl<const N: usize> IntoResponse for [u8; N] {
    #[inline]
    fn into_response(self) -> Response {
        self.to_vec().into_response()
    }
}

impl<const N: usize> IntoResponse for &'static [u8; N] {
    #[inline]
    fn into_response(self) -> Response {
        self.as_slice().into_response()
    }
}
