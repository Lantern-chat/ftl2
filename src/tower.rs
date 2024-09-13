use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
};

use crate::service::Service as FtlService;
use crate::Request;
use futures::FutureExt;
use tower_layer::Layer;
use tower_service::Service as TowerService;

/// Wrapper for a [`tower::Service`](tower_service::Service) to
/// be used as an [`ftl::Service`](FtlService).
///
/// This is accomplished by wrapping it in a [`Mutex`] to use
/// the `&mut self` methods of [`tower::Service`](tower_service::Service).
pub struct TowerServiceOrFtlService<S> {
    pub inner: Mutex<S>,
}

impl<S> TowerServiceOrFtlService<S> {
    pub const fn new(inner: S) -> Self {
        Self {
            inner: Mutex::new(inner),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct TowerLayer<L>(pub L);

impl<S> FtlService<Request> for TowerServiceOrFtlService<S>
where
    S: TowerService<Request, Future: Send + 'static> + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.lock().unwrap().poll_ready(cx)
    }

    fn call(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = Result<Self::Response, Self::Error>> + Send + 'static
    {
        self.inner.lock().unwrap().call(req)
    }
}

// impl<S> TowerService<Request> for TowerServiceOrFtlService<S>
// where
//     S: FtlService<Request> + Send + 'static,
// {
//     type Response = S::Response;
//     type Error = S::Error;
//     type Future =
//         Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

//     fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         Mutex::get_mut(&mut self.inner).unwrap().poll_ready(cx)
//     }

//     fn call(&mut self, req: Request) -> Self::Future {
//         Mutex::get_mut(&mut self.inner).unwrap().call(req).boxed()
//     }
// }

impl<L, S> Layer<S> for TowerLayer<L>
where
    L: Layer<S>,
    <L as Layer<S>>::Service: TowerService<Request, Future: Send + 'static> + Send + 'static,
    TowerServiceOrFtlService<L::Service>: FtlService<Request>,
{
    type Service = TowerServiceOrFtlService<L::Service>;

    fn layer(&self, inner: S) -> Self::Service {
        TowerServiceOrFtlService::new(self.0.layer(inner))
    }
}

// pub fn ftl_service_as_tower_service<S, Req>(
//     service: S,
// ) -> impl TowerService<Req, Response = S::Response, Error = S::Error> + Send + 'static
// where
//     S: FtlService<Req> + 'static,
// {
//     FtlServiceAsTowerService {
//         service,
//         f: PhantomData,
//     }
// }

// pub struct FtlServiceAsTowerService<S, F> {
//     service: S,
//     f: PhantomData<F>,
// }

// impl<S, F, Fut, Req, R, E> TowerService<Req> for FtlServiceAsTowerService<S, F>
// where
//     S: FtlService<Req, Error = E, Response = R>,
//     F: Fn(&S, Req) -> Fut + 'static,
//     Fut: Future<Output = Result<R, E>> + Send + 'static,
// {
//     type Response = R;
//     type Error = E;
//     type Future = Fut;

//     fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), E>> {
//         self.service.poll_ready(cx)
//     }

//     fn call(&mut self, req: Req) -> Self::Future {
//         self.service.call(req)
//     }
// }
