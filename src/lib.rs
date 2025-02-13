#![warn(clippy::perf, clippy::style, clippy::must_use_candidate)]
#![allow(clippy::manual_async_fn, non_local_definitions)]

extern crate tracing as log;

#[doc(hidden)]
pub extern crate paste;

pub extern crate http;

#[macro_use]
mod macros;

pub mod body;
pub mod error;
pub mod extract;
pub mod handler;
pub mod headers;
pub mod layers;
pub mod params;
pub mod response;
pub mod rewrite;
pub mod router;
pub mod serve;
pub mod service;

#[cfg(feature = "ws")]
pub mod ws;

#[cfg(feature = "fs")]
pub mod fs;

pub use http::request::Parts as RequestParts;
pub use http::response::Parts as ResponseParts;
pub type Request = http::Request<body::Body>;
pub type Response = http::Response<body::Body>;

pub use crate::error::Error;
pub use crate::extract::FromRequest;
pub use crate::layers::Layer;
pub use crate::response::IntoResponse;
pub use crate::router::Router;
pub use crate::service::Service;

#[cfg(feature = "tower-service")]
pub mod tower;

// sonic-rs advertises itself as being faster only on x86_64 and aarch64, so we
// use it on those platforms. Otherwise, we use serde_json.
#[cfg(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
extern crate sonic_rs as json_impl;

#[cfg(all(
    feature = "json",
    not(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64")))
))]
extern crate serde_json as json_impl;

// serde_html_form is a fork of serde_urlencoded that supports more
// more formats, but might have slight compatibility issues, so make it opt-in.
#[cfg(feature = "serde_html_form")]
pub(crate) use serde_html_form as form_impl;
#[cfg(not(feature = "serde_html_form"))]
pub(crate) use serde_urlencoded as form_impl;
