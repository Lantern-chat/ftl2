use std::io::{self, ErrorKind};

use http::StatusCode;

use crate::{body::BodyError, IntoResponse};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Hyper error: {0}")]
    HyperError(#[from] hyper::Error),

    #[error(transparent)]
    IoError(#[from] io::Error),

    #[error(transparent)]
    BodyError(#[from] BodyError),

    #[error(transparent)]
    Utf8Error(#[from] std::str::Utf8Error),

    #[error("Stream Aborted")]
    StreamAborted,

    #[error("Bad Request")]
    BadRequest,

    #[error("Missing header: {0}")]
    MissingHeader(&'static str),

    #[error("Invalid header: {0}: {1}")]
    InvalidHeader(&'static str, headers::Error),

    #[error("Not Found")]
    NotFound,

    #[error("Request timed out")]
    TimedOut,

    #[error("Unauthorized")]
    Unauthorized,

    #[error("The request method is not allowed")]
    MethodNotAllowed,

    #[error("Unsupported media type")]
    UnsupportedMediaType,

    #[error("Payload too large")]
    PayloadTooLarge,

    #[error("The request is missing a required extension")]
    MissingExtension,

    #[error("The query is missing")]
    MissingQuery,

    #[error("The request is missing a matched path")]
    MissingMatchedPath,

    #[error(transparent)]
    Form(#[from] crate::form_impl::de::Error),

    #[cfg(feature = "cbor")]
    #[error("An error occurred while reading the body: {0}")]
    Cbor(#[from] ciborium::de::Error<std::io::Error>),

    #[cfg(feature = "json")]
    #[error(transparent)]
    Json(#[from] json_impl::Error),

    #[error("Path error: {0}")]
    Path(#[from] crate::extract::path::PathError),

    #[error("Scheme error: {0}")]
    Scheme(#[from] crate::extract::scheme::SchemeError),

    #[error("Authority error: {0}")]
    Authority(#[from] crate::extract::AuthorityError),

    #[cfg(feature = "ws")]
    #[error("Websocket error: {0}")]
    WebsocketError(#[from] crate::ws::WsError),

    #[error("Custom error: {0}")]
    Custom(Box<dyn core::error::Error + Send + Sync + 'static>),
}

pub type BoxError = Box<dyn core::error::Error + Send + Sync>;

#[allow(dead_code)] // only used when certain features are enabled
pub(crate) fn io_other<E: Into<BoxError>>(error: E) -> io::Error {
    io::Error::new(ErrorKind::Other, error)
}

impl IntoResponse for Error {
    fn into_response(self) -> crate::Response {
        match self {
            Error::HyperError(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            Error::IoError(e) => {
                // TODO: More kinds
                if e.kind() == ErrorKind::NotFound {
                    return StatusCode::NOT_FOUND.into_response();
                }
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
            Error::BodyError(e) => e.into_response(),
            Error::Utf8Error(e) => {
                (format!("The body was not valid UTF-8: {e}"), StatusCode::BAD_REQUEST).into_response()
            }
            Error::StreamAborted => {
                (String::from("Stream Aborted"), http::StatusCode::BAD_REQUEST).into_response()
            }
            Error::Form(_) => StatusCode::BAD_REQUEST.into_response(),
            Error::NotFound => StatusCode::NOT_FOUND.into_response(),
            Error::TimedOut => StatusCode::REQUEST_TIMEOUT.into_response(),
            Error::Unauthorized => StatusCode::UNAUTHORIZED.into_response(),
            Error::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
            Error::MissingExtension => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            Error::BadRequest => StatusCode::BAD_REQUEST.into_response(),
            Error::MissingHeader(e) => {
                (format!("Missing header(s): {e}"), StatusCode::BAD_REQUEST).into_response()
            }
            Error::InvalidHeader(h, error) => {
                (format!("Invalid header: {h}: {error}"), StatusCode::BAD_REQUEST).into_response()
            }
            Error::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            Error::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response(),
            Error::MissingQuery => ("Missing URI query", StatusCode::BAD_REQUEST).into_response(),
            Error::MissingMatchedPath => ("Missing matched path", StatusCode::BAD_REQUEST).into_response(),

            #[cfg(feature = "cbor")]
            Error::Cbor(error) => {
                (format!("Error parsing CBOR body: {error}"), StatusCode::BAD_REQUEST).into_response()
            }
            #[cfg(feature = "json")]
            Error::Json(error) => {
                (format!("Error parsing JSON body: {error}"), StatusCode::BAD_REQUEST).into_response()
            }
            Error::Path(path_error) => (path_error.to_string(), StatusCode::BAD_REQUEST).into_response(),
            Error::Scheme(scheme_error) => scheme_error.into_response(),
            Error::Authority(authority_error) => authority_error.into_response(),

            #[cfg(feature = "ws")]
            Error::WebsocketError(ws_error) => ws_error.into_response(),

            Error::Custom(e) => {
                log::error!("Custom error: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

impl From<core::convert::Infallible> for Error {
    #[inline(always)]
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}
