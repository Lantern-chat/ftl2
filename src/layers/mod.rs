pub use tower_layer::{layer_fn, Identity, LayerFn, Stack};

pub use crate::extract::real_ip::RealIpLayer;

pub mod convert_body;
pub mod handle_error;
pub mod method_override;

#[cfg(feature = "gcra")]
pub mod rate_limit;

#[cfg(feature = "_meta_compression")]
pub mod compression;

/// Decorates a [`Service`](crate::Service), transforming either the request or the response.
/// This is re-exported from the [`tower_layer`] crate, but is used
/// differently here.
///
/// Any below examples generated from `tower_layer::Layer` about using tower services will
/// be incorrect as it is used for `ftl`.
///
/// |
///
/// |
///
/// |
///
/// |
///
/// |
///
/// Begin rexported docs:
///
pub use tower_layer::Layer;
