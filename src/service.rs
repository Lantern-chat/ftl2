use std::{
    error::Error,
    future::Future,
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
    task::{Context, Poll},
};

use crate::{response::IntoResponse, Request};

pub trait Service<Req>: Send + Sync + 'static {
    type Response;
    type Error;

    #[cfg(feature = "tower-service")]
    fn poll_ready(&self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;

    fn call(
        &self,
        req: Req,
    ) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send + 'static;
}

// impl<F, Req, Fut, R, E> Service<Req> for F
// where
//     F: Fn(Req) -> Fut + Send + Sync + 'static,
//     Fut: Future<Output = Result<R, E>> + Send + 'static,
// {
//     type Response = R;
//     type Error = E;

//     fn call(
//         &self,
//         req: Req,
//     ) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send + 'static {
//         (self)(req)
//     }
// }

impl<R, T> Service<R> for T
where
    T: Deref<Target: Service<R>> + Send + Sync + 'static,
{
    type Response = <<T as Deref>::Target as Service<R>>::Response;
    type Error = <<T as Deref>::Target as Service<R>>::Error;

    #[cfg(feature = "tower-service")]
    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        (**self).poll_ready(cx)
    }

    fn call(
        &self,
        req: R,
    ) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send + 'static {
        (**self).call(req)
    }
}

pub trait MakeService<Target, Request> {
    type Service: Service<Request, Error: Into<crate::error::BoxError>> + Send;

    fn make_service(&self, target: Target) -> Self::Service;
}

pub struct MakeServiceFn<F>(pub F);

impl<F, S, Target, Request> MakeService<Target, Request> for MakeServiceFn<F>
where
    F: Fn(Target) -> S,
    S: Service<Request, Error: Into<crate::error::BoxError>> + Send,
{
    type Service = S;

    fn make_service(&self, target: Target) -> Self::Service {
        (self.0)(target)
    }
}

pub struct MapServiceRequest<S, F> {
    service: S,
    f: F,
}

impl<S, F, Req1, Req2> Service<Req1> for MapServiceRequest<S, F>
where
    S: Service<Req2>,
    F: Fn(Req1) -> Req2 + Send + Sync + 'static,
{
    type Response = S::Response;
    type Error = S::Error;

    #[cfg(feature = "tower-service")]
    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(
        &self,
        req: Req1,
    ) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send + 'static {
        self.service.call((self.f)(req))
    }
}

impl<S, F> MapServiceRequest<S, F> {
    pub fn new(service: S, f: F) -> Self {
        Self { service, f }
    }
}

#[derive(Clone)]
#[repr(transparent)]
pub struct FtlServiceToHyperMakeService<S>(Arc<S>)
where
    S: Service<http::Request<hyper::body::Incoming>, Error: Error + Send + Sync + 'static> + Send;

impl<S> FtlServiceToHyperMakeService<S>
where
    S: Service<http::Request<hyper::body::Incoming>, Error: Error + Send + Sync + 'static> + Send,
{
    pub fn new(service: S) -> Self {
        Self(Arc::new(service))
    }
}

impl<S, Target> MakeService<Target, http::Request<hyper::body::Incoming>>
    for FtlServiceToHyperMakeService<S>
where
    S: Service<http::Request<hyper::body::Incoming>, Error: Error + Send + Sync + 'static> + Send,
{
    type Service = Arc<S>;

    fn make_service(&self, _target: Target) -> Self::Service {
        self.0.clone()
    }
}
