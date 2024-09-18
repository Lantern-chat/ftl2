use core::str::FromStr;
use http::{
    uri::{InvalidUri, Scheme},
    HeaderMap, HeaderName,
};

use crate::{IntoResponse, Response};

use super::FromRequestParts;

#[derive(Debug, thiserror::Error)]
pub enum SchemeRejection {
    #[error("Invalid scheme: {0}")]
    InvalidScheme(#[from] InvalidUri),

    #[error("Missing scheme")]
    MissingScheme,
}

impl IntoResponse for SchemeRejection {
    fn into_response(self) -> Response {
        match self {
            SchemeRejection::InvalidScheme(_) => ("Invalid Scheme", http::StatusCode::BAD_REQUEST).into_response(),
            SchemeRejection::MissingScheme => ("Missing Scheme", http::StatusCode::BAD_REQUEST).into_response(),
        }
    }
}

impl<S> FromRequestParts<S> for Scheme {
    type Rejection = SchemeRejection;

    fn from_request_parts(
        parts: &mut http::request::Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        // borrowed from https://github.com/tokio-rs/axum/pull/2507
        fn parse_forwarded(headers: &HeaderMap) -> Option<&str> {
            // if there are multiple `Forwarded` `HeaderMap::get` will return the first one
            let forwarded_values = headers.get(http::header::FORWARDED)?.to_str().ok()?;

            // get the first set of values
            let first_value = forwarded_values.split(',').next()?;

            // find the value of the `proto` field
            first_value.split(';').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                key.trim().eq_ignore_ascii_case("proto").then(|| value.trim().trim_matches('"'))
            })
        }

        async move {
            if let Some(scheme) = parse_forwarded(&parts.headers) {
                return Ok(Scheme::from_str(scheme)?);
            }

            // X-Forwarded-Proto
            if let Some(scheme) = parts
                .headers
                .get(HeaderName::from_static("x-forwarded-proto"))
                .map(|scheme| Scheme::try_from(scheme.as_bytes()))
                .transpose()?
            {
                return Ok(scheme);
            }

            // From parts of an HTTP/2 request
            if let Some(scheme) = parts.uri.scheme() {
                return Ok(scheme.clone());
            }

            Err(SchemeRejection::MissingScheme)
        }
    }
}
