use crate::{body::Body, service::ServiceFuture, Layer, Request, Service};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ConvertBody<S = ()>(pub S);

impl<S> Layer<S> for ConvertBody<()> {
    type Service = ConvertBody<S>;

    fn layer(&self, service: S) -> Self::Service {
        ConvertBody(service)
    }
}

impl<S, B> Service<http::Request<B>> for ConvertBody<S>
where
    S: Service<Request>,
    B: http_body::Body<Data = bytes::Bytes, Error: std::error::Error + Send + Sync + 'static> + Send + 'static,
{
    type Error = S::Error;
    type Response = S::Response;

    #[inline]
    fn call(&self, req: http::Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        let (parts, body) = req.into_parts();

        self.0.call(Request::from_parts(parts, Body::from_any_body(body)))
    }
}
