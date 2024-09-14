#![allow(private_interfaces)]

use core::str::FromStr;
use std::{error::Error, future::Future, sync::Arc};

use http::{request::Parts, StatusCode};

use crate::{params::UrlParams, response::IntoResponse, Response};

use super::FromRequestParts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Path<P: PathSegments>(pub P::Output);

pub trait PathSegments: Send + 'static {
    type Output: Send + 'static;

    #[doc(hidden)]
    fn parse_segments(segments: &UrlParams) -> Result<Self::Output, PathRejection>;
}

pub trait PathSegment: Send + 'static {
    const NAME: &'static str;
    type Type: FromStr<Err: Error> + Send + 'static;
}

impl<T> PathSegments for T
where
    T: PathSegment,
{
    type Output = T::Type;

    fn parse_segments(segments: &UrlParams) -> Result<Self::Output, PathRejection> {
        let segment = match segments {
            UrlParams::InvalidUtf8InPathParam { key } => {
                return Err(PathRejection::InvalidUtf8InPathParam { key: key.clone() });
            }
            UrlParams::Params(params) => {
                params.iter().find_map(|(k, v)| if k.as_ref() == T::NAME { Some(v) } else { None })
            }
        };

        let segment = segment.ok_or(PathRejection::MissingSegment(T::NAME))?;

        match FromStr::from_str(&segment.0) {
            Ok(value) => Ok(value),
            Err(e) => Err(PathRejection::InvalidSegment(e.to_string())),
        }
    }
}

/// Defines a path segment that can be parsed from the URL path.
#[macro_export]
macro_rules! path_segment {
    ($($vis:vis $name:ident $(as $alt:literal)?: $ty:ty),* $(,)?) => {$crate::paste::paste!{$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis struct $name;

        impl $crate::extract::path::PathSegment for $name {
            const NAME: &'static str = ($($alt,)? stringify!([<$name:snake>]),).0;
            type Type = $ty;
        }
    )*}};
}

macro_rules! decl_segments {
    ($($t:ident),*) => {
        impl<$($t: $crate::extract::path::PathSegment,)*> $crate::extract::path::PathSegments for ($($t,)*) {
            type Output = ($($t::Type,)*);

            fn parse_segments(segments: &$crate::params::UrlParams) -> Result<Self::Output, $crate::extract::path::PathRejection> {
                Ok(($($t::parse_segments(segments)?,)*))
            }
        }
    };
}

all_the_tuples_no_last_special_case!(decl_segments);

#[derive(Debug, thiserror::Error)]
pub enum PathRejection {
    #[error("missing path parameters")]
    MissingParameters,

    #[error("missing segment: {0}")]
    MissingSegment(&'static str),

    #[error("invalid segment: {0}")]
    InvalidSegment(String),

    #[error("invalid UTF-8 in path parameter: {key}")]
    InvalidUtf8InPathParam { key: Arc<str> },
}

impl IntoResponse for PathRejection {
    fn into_response(self) -> Response {
        (self.to_string(), StatusCode::BAD_REQUEST).into_response()
    }
}

impl<P, S> FromRequestParts<S> for Path<P>
where
    P: PathSegments,
{
    type Rejection = PathRejection;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let params = parts.extensions.get::<UrlParams>().ok_or(PathRejection::MissingParameters);

        async move { Ok(Path(P::parse_segments(params?)?)) }
    }
}

#[cfg(test)]
mod tests {
    use crate::{body::Body, params::PercentDecodedStr};

    use super::*;

    #[tokio::test]
    async fn test_path() {
        // https://example.com/{user_id}/{party_id}/{something}

        // you define these manually before declaring your routes
        path_segment! {
            pub UserId as "user_id": u8,
            pub PartyId: u16,
            pub Something: u32,
        }

        // this is done automatically during routing, just here to simulate it for testing
        #[rustfmt::skip]
        let (mut parts, _) = http::request::Builder::new().extension(UrlParams::Params(vec![
            (Arc::from("user_id"), PercentDecodedStr::new("1").unwrap()),
            (Arc::from("party_id"), PercentDecodedStr::new("2").unwrap()),
            (Arc::from("something"), PercentDecodedStr::new("3").unwrap()),
        ])).body(Body::empty()).unwrap().into_parts();

        // this is what you would do in your handler
        fn fixture(Path((user_id, party_id, something)): Path<(UserId, PartyId, Something)>) {
            assert_eq!(user_id, 1);
            assert_eq!(party_id, 2);
            assert_eq!(something, 3);
        }

        // test the handler
        fixture(Path::from_request_parts(&mut parts, &()).await.unwrap());
    }
}
