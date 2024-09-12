#![allow(clippy::manual_async_fn, unused_imports)]

extern crate tracing as log;

#[doc(hidden)]
pub extern crate paste;

#[macro_use]
mod macros;

pub mod body;
pub mod error;
pub mod extract;
pub mod handler;
pub mod headers;
pub mod params;
pub mod response;
pub mod router;
pub mod serve;
pub mod service;
pub mod ws;

pub type Request = http::Request<body::Body>;
pub type Response = http::Response<body::Body>;

use crate::extract::FromRequest;
use crate::response::IntoResponse;

#[cfg(feature = "tower-service")]
pub mod tower;
