use std::convert::Infallible;

use crate::{RequestParts, Service};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RedirectKind {
    Permanent,
    Temporary,
}

/// Helper service that redirects all using the provided function.
///
/// This service is useful for redirecting all HTTP traffic to HTTPS.
///
/// This service will automatically inject the [Authority](http::uri::Authority) into the request extensions,
/// parsed from both the `Host` header and parsed URI.
#[derive(Clone)]
pub struct RewriteService<F>(RedirectKind, F);

impl<F> RewriteService<F>
where
    F: Fn(&RequestParts) -> String + Clone + Send + Sync + 'static,
{
    pub const fn new(kind: RedirectKind, f: F) -> Self {
        Self(kind, f)
    }

    pub const fn permanent(f: F) -> Self {
        Self(RedirectKind::Permanent, f)
    }

    pub const fn temporary(f: F) -> Self {
        Self(RedirectKind::Temporary, f)
    }
}

impl<F, B> Service<http::Request<B>> for RewriteService<F>
where
    F: Fn(&RequestParts) -> String + Clone + Send + Sync + 'static,
{
    type Response = http::Response<http_body_util::Empty<bytes::Bytes>>;
    type Error = Infallible;

    #[inline]
    fn call(&self, req: http::Request<B>) -> impl crate::service::ServiceFuture<Self::Response, Self::Error> {
        let (mut parts, body) = req.into_parts();

        drop(body); // explicitly drop the body

        if let Ok(authority) = crate::extract::extract_authority(&parts) {
            parts.extensions.insert(authority);
        }

        let status = match self.0 {
            RedirectKind::Permanent => http::StatusCode::MOVED_PERMANENTLY,
            RedirectKind::Temporary => http::StatusCode::FOUND,
        };

        std::future::ready(Ok(http::Response::builder()
            .header(http::header::LOCATION, (self.1)(&parts))
            .status(status)
            .body(Default::default())
            .unwrap()))
    }
}
