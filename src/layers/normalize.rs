use std::convert::Infallible;

use futures::FutureExt as _;
use http::{header, HeaderMap, HeaderName, HeaderValue, Method};
use http_body::Body as _;

use crate::{body::Body, service::ServiceFuture, IntoResponse, Layer, Response, Service};

/// Normalizes the response by ensuring that the `Content-Length` header is set
/// and the body is empty for `HEAD` requests and `CONNECT` responses.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Normalize<S = ()>(pub S);

impl<S> Layer<S> for Normalize {
    type Service = Normalize<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Normalize(inner)
    }
}

impl<S, B> Service<http::Request<B>> for Normalize<S>
where
    S: Service<http::Request<B>, Response: IntoResponse, Error: IntoResponse>,
{
    type Response = Response;
    type Error = Infallible;

    fn call(&self, mut req: http::Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        // This is sometimes used in old browsers without support for PATCH or OPTIONS methods.
        if let Some(method_override) =
            req.headers().get(const { HeaderName::from_static("x-http-method-override") })
        {
            if let Ok(method_override) = Method::from_bytes(method_override.as_bytes()) {
                *req.method_mut() = method_override;
            }
        }

        let method = match *req.method() {
            Method::HEAD => MiniMethod::Head,
            Method::CONNECT => MiniMethod::Connect,
            _ => MiniMethod::Other,
        };

        self.0.call(req).map(move |res| {
            let mut resp = res.into_response();

            if method == MiniMethod::Connect && resp.status().is_success() {
                // From https://httpwg.org/specs/rfc9110.html#CONNECT:
                // > A server MUST NOT send any Transfer-Encoding or
                // > Content-Length header fields in a 2xx (Successful)
                // > response to CONNECT.
                if resp.headers().contains_key(header::CONTENT_LENGTH)
                    || resp.headers().contains_key(header::TRANSFER_ENCODING)
                    || resp.size_hint().lower() != 0
                {
                    log::error!("response to CONNECT with nonempty body");
                    resp = resp.map(|_| Body::empty());
                }
            } else {
                // make sure to set content-length before removing the body
                set_content_length(resp.size_hint(), resp.headers_mut());
            }

            if method == MiniMethod::Head {
                resp = resp.map(|_| Body::empty());
            }

            // http://www.gnuterrypratchett.com/
            resp.headers_mut().insert(
                http::HeaderName::from_static("x-clacks-overhead"),
                http::HeaderValue::from_static("GNU Terry Pratchett"),
            );

            Ok(resp)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiniMethod {
    Head,
    Connect,
    Other,
}

fn set_content_length(size_hint: http_body::SizeHint, headers: &mut HeaderMap) {
    if headers.contains_key(header::CONTENT_LENGTH) {
        return;
    }

    if let Some(size) = size_hint.exact() {
        let header_value = if size == 0 {
            const { HeaderValue::from_static("0") }
        } else {
            let mut buffer = itoa::Buffer::new();
            HeaderValue::from_str(buffer.format(size)).unwrap()
        };

        headers.insert(header::CONTENT_LENGTH, header_value);
    }
}
