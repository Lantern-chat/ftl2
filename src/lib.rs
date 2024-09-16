#![warn(clippy::perf, clippy::style, clippy::must_use_candidate)]
#![allow(clippy::manual_async_fn)]

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
pub mod ws;

pub use http::request::Parts as RequestParts;
pub use http::response::Parts as ResponseParts;
pub type Request = http::Request<body::Body>;
pub type Response = http::Response<body::Body>;

pub use crate::extract::FromRequest;
pub use crate::response::IntoResponse;
pub use crate::router::Router;
pub use crate::service::Service;
pub use tower_layer::Layer;

#[cfg(feature = "tower-service")]
pub mod tower;

// sonic-rs advertises itself as being faster only on x86_64 and aarch64, so we
// use it on those platforms. Otherwise, we use serde_json.
#[cfg(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
extern crate sonic_rs as json_impl;

#[cfg(not(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64"))))]
extern crate serde_json as json_impl;
