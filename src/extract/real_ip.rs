//! Provides a way to get the real IP address of the client, to the best of our ability.
//!
//! This is exposed as [`RealIp`], which is an extractor that can be used in handler parameters,
//! or as a [`Layer`] and [`Service`] that can be used to add the IP address to
//! the request parts extensions. Or both.

use std::{
    fmt::{self, Debug, Display},
    future::{self, Future},
    hash::Hash,
    net::{IpAddr, SocketAddr},
    ops::Deref,
    str::FromStr,
};

use crate::{extract::FromRequestParts, service::ServiceFuture, Layer, RequestParts, Service};
use http::{header::HeaderName, Extensions, HeaderMap, HeaderValue, Request};

/// Wrapper around [`std::net::IpAddr`] that can be extracted from the request parts.
///
/// This extractor will try to get the real IP address of the client, using the following headers, in order:
/// - `cf-connecting-ip` (used by Cloudflare sometimes)
/// - `x-cluster-client-ip` (used by AWS sometimes)
/// - `fly-client-ip` (used by Fly.io sometimes)
/// - `fastly-client-ip` (used by Fastly sometimes)
/// - `cloudfront-viewer-address" (used by Cloudfront sometimes)
/// - `x-real-ip`
/// - `x-forwarded-for`
/// - `x-original-forwarded-for` (maybe used by Cloudfront?)
/// - `true-client-ip` (used by some load balancers)
/// - `client-ip` (used by some load balancers)
///
/// If none of these headers are found, it will return a 400 Bad Request via [`IpAddrRejection`].
///
/// The [`RealIpLayer`] can be also used to add the [`RealIp`] extension to the request if available, allowing
/// other services or extractors to reuse it without rescanning the headers every time.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RealIp(pub IpAddr);

/// Like [`RealIp`], but with the last 64 bits of any IPv6 address set to zeroes.
///
/// This is useful for making sure clients with randomized IPv6 interfaces
/// aren't treated as different clients. This can be common in some networks
/// that attempt to preserve privacy.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RealIpPrivacyMask(pub RealIp);

impl From<RealIp> for RealIpPrivacyMask {
    #[inline]
    fn from(ip: RealIp) -> Self {
        RealIpPrivacyMask(match ip.0 {
            IpAddr::V4(ip) => RealIp(IpAddr::V4(ip)),
            IpAddr::V6(ip) => RealIp(IpAddr::V6(From::from(
                ip.to_bits() & 0xFFFF_FFFF_FFFF_FFFF_0000_0000_0000_0000,
            ))),
        })
    }
}

impl From<RealIpPrivacyMask> for RealIp {
    #[inline]
    fn from(mask: RealIpPrivacyMask) -> Self {
        mask.0
    }
}

impl From<RealIp> for IpAddr {
    #[inline]
    fn from(ip: RealIp) -> Self {
        ip.0
    }
}

impl From<RealIpPrivacyMask> for IpAddr {
    #[inline]
    fn from(mask: RealIpPrivacyMask) -> Self {
        mask.0.into()
    }
}

impl Debug for RealIp {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl Display for RealIp {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Debug for RealIpPrivacyMask {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl Display for RealIpPrivacyMask {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl Deref for RealIp {
    type Target = IpAddr;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for RealIpPrivacyMask {
    type Target = RealIp;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for RealIp {
    type Rejection = crate::Error;

    fn from_request_parts(
        parts: &mut RequestParts,
        _: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        future::ready(
            parts
                .extensions
                .get::<RealIp>()
                .cloned()
                .or_else(|| get_ip_from_parts(parts))
                .ok_or(crate::Error::BadRequest),
        )
    }
}

impl<S> FromRequestParts<S> for RealIpPrivacyMask {
    type Rejection = crate::Error;

    fn from_request_parts(
        parts: &mut RequestParts,
        _: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        future::ready(
            match parts.extensions.get::<RealIp>().cloned().or_else(|| get_ip_from_parts(parts)) {
                Some(ip) => Ok(ip.into()),
                None => Err(crate::Error::BadRequest),
            },
        )
    }
}

/// [`Layer`]/[`Service`] that adds the [`RealIp`] extension to the request parts if available.
///
/// This extension can then be reused by other services or extractors, such as [`RealIp`] itself.
#[derive(Default, Debug, Clone, Copy)]
pub struct RealIpLayer<S = ()>(pub S);

impl<B, I> Service<Request<B>> for RealIpLayer<I>
where
    I: Service<Request<B>>,
{
    type Response = I::Response;
    type Error = I::Error;

    #[inline]
    fn call(&self, mut req: Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        if let Some(ip) = get_ip_from_headers(req.headers(), req.extensions()) {
            req.extensions_mut().insert(ip);
        }

        self.0.call(req)
    }
}

impl<I> Layer<I> for RealIpLayer {
    type Service = RealIpLayer<I>;

    fn layer(&self, inner: I) -> Self::Service {
        RealIpLayer(inner)
    }
}

pub(crate) fn get_ip_from_headers(headers: &HeaderMap, extensions: &Extensions) -> Option<RealIp> {
    fn parse_ip(s: &HeaderValue) -> Option<IpAddr> {
        s.to_str()
            .ok()
            .and_then(|s| s.split(&[',', ':']).next())
            .and_then(|s| IpAddr::from_str(s.trim()).ok())
    }

    static HEADERS: [HeaderName; 10] = [
        HeaderName::from_static("cf-connecting-ip"), // used by Cloudflare sometimes
        HeaderName::from_static("x-cluster-client-ip"), // used by AWS sometimes
        HeaderName::from_static("fly-client-ip"),    // used by Fly.io sometimes
        HeaderName::from_static("fastly-client-ip"), // used by Fastly sometimes
        HeaderName::from_static("cloudfront-viewer-address"), // used by Cloudfront sometimes
        HeaderName::from_static("x-real-ip"),
        HeaderName::from_static("x-forwarded-for"),
        HeaderName::from_static("x-original-forwarded-for"), // maybe used by Cloudfront?
        HeaderName::from_static("true-client-ip"),           // used by some load balancers
        HeaderName::from_static("client-ip"),                // used by some load balancers
    ];

    for header in &HEADERS {
        if let Some(real_ip) = headers.get(header).and_then(parse_ip) {
            return Some(RealIp(real_ip));
        }
    }

    // fallback to the socket address
    if let Some(info) = extensions.get::<SocketAddr>() {
        return Some(RealIp(info.ip()));
    }

    None
}

#[inline]
pub(crate) fn get_ip_from_parts(parts: &RequestParts) -> Option<RealIp> {
    get_ip_from_headers(&parts.headers, &parts.extensions)
}
