use std::time::Instant;

use crate::{
    headers::server_timing::{ServerTiming, ServerTimings},
    service::{Service, ServiceFuture},
    Layer,
};

use futures::TryFutureExt as _;
use headers::HeaderMapExt as _;

/// A [`Layer`] that adds a `Server-Timing` header to the response with the
/// duration of the request.
#[derive(Default, Debug, Clone, Copy)]
#[repr(transparent)]
pub struct RespTimingLayer<S = ()>(pub S);

impl<S> Layer<S> for RespTimingLayer {
    type Service = RespTimingLayer<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RespTimingLayer(inner)
    }
}

/// Request Extension to store the start time of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct StartTime(pub Instant);

impl<ReqBody, ResBody, S> Service<http::Request<ReqBody>> for RespTimingLayer<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;

    #[inline]
    fn call(&self, mut req: http::Request<ReqBody>) -> impl ServiceFuture<Self::Response, Self::Error> {
        let start = Instant::now();

        req.extensions_mut().insert(StartTime(start));

        self.0.call(req).map_ok(move |mut resp| {
            let timing = ServerTiming::new("resp").elapsed_from(start);

            resp.headers_mut().typed_insert(ServerTimings::new().with(timing));

            resp
        })
    }
}
