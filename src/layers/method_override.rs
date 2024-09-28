use http::HeaderName;

use crate::{service::ServiceFuture, Layer, Service};

/// If the `x-http-method-override` header is present and contains a valid HTTP method,
/// this layer will override the request's method with the one specified in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct MethodOverride<S = ()>(pub S);

impl<S> Layer<S> for MethodOverride {
    type Service = MethodOverride<S>;

    #[inline]
    fn layer(&self, service: S) -> Self::Service {
        MethodOverride(service)
    }
}

impl<S, B> Service<http::Request<B>> for MethodOverride<S>
where
    S: Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;

    #[inline]
    fn call(&self, mut req: http::Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        if let Some(method) = req.headers().get(const { HeaderName::from_static("x-http-method-override") }) {
            if let Ok(method) = http::Method::from_bytes(method.as_bytes()) {
                *req.method_mut() = method;
            }
        }

        self.0.call(req)
    }
}
