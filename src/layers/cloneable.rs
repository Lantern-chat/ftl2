use std::sync::Arc;

use crate::{
    service::{Service, ServiceFuture},
    Layer,
};

/// Simple layer that wraps a service in an `Arc` to allow cloning.
/// This does not affect the service itself.
#[derive(Debug, Default)]
#[repr(transparent)]
pub struct Cloneable<S = ()>(pub Arc<S>);

impl<S> Clone for Cloneable<S> {
    fn clone(&self) -> Self {
        Cloneable(self.0.clone())
    }
}

impl<S> Layer<S> for Cloneable {
    type Service = Cloneable<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Cloneable(Arc::new(inner))
    }
}

impl<S, Req> Service<Req> for Cloneable<S>
where
    S: Service<Req>,
{
    type Response = S::Response;
    type Error = S::Error;

    #[inline(always)]
    fn call(&self, req: Req) -> impl ServiceFuture<Self::Response, Self::Error> {
        self.0.call(req)
    }
}
