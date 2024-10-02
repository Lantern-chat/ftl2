use futures::FutureExt as _;

use crate::{
    service::{Service, ServiceFuture},
    Layer,
};

/// Spawns each request on its own task and catches any panics as internal server errors.
#[derive(Debug, Clone, Copy, Default)]
#[repr(transparent)]
pub struct CatchPanic<S = ()>(pub S);

impl<S> Layer<S> for CatchPanic {
    type Service = CatchPanic<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CatchPanic(inner)
    }
}

impl<S, Req, ResBody> Service<Req> for CatchPanic<S>
where
    S: Service<Req, Response = http::Response<ResBody>> + Clone + 'static,
    ResBody: Default + Send + 'static,
    Req: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;

    fn call(&self, req: Req) -> impl ServiceFuture<Self::Response, Self::Error> {
        let inner = self.0.clone();

        tokio::task::spawn(async move { inner.call(req).await }).map(|res| match res {
            Ok(res) => res,
            Err(err) => {
                log::error!("Service panicked: {:?}", err);

                let mut resp = http::Response::new(ResBody::default());
                *resp.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
                Ok(resp)
            }
        })
    }
}
