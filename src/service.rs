use std::{error::Error, future::Future, ops::Deref};

pub trait ServiceFuture<R, E>: Future<Output = Result<R, E>> + Send {}

impl<T, R, E> ServiceFuture<R, E> for T where T: Future<Output = Result<R, E>> + Send {}

pub trait Service<Req>: Send + Sync {
    type Response;
    type Error: Send + 'static;

    fn call(&self, req: Req) -> impl ServiceFuture<Self::Response, Self::Error>;
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
//     ) -> impl ServiceFuture<Self::Response, Self::Error> {
//         (self)(req)
//     }
// }

impl<R, T> Service<R> for T
where
    T: Deref<Target: Service<R>> + Send + Sync,
{
    type Response = <<T as Deref>::Target as Service<R>>::Response;
    type Error = <<T as Deref>::Target as Service<R>>::Error;

    #[inline]
    fn call(&self, req: R) -> impl ServiceFuture<Self::Response, Self::Error> {
        (**self).call(req)
    }
}

pub trait MakeService<Target, Request> {
    type Service: Service<Request, Error: Error + Send + Sync + 'static> + Send;

    fn make_service(&self, target: Target) -> Self::Service;
}

pub struct MakeServiceFn<F>(pub F);

impl<F, S, Target, Request> MakeService<Target, Request> for MakeServiceFn<F>
where
    F: Fn(Target) -> S,
    S: Service<Request, Error: Error + Send + Sync + 'static> + Send,
{
    type Service = S;

    #[inline]
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
    F: Fn(Req1) -> Req2 + Send + Sync,
{
    type Response = S::Response;
    type Error = S::Error;

    #[inline]
    fn call(&self, req: Req1) -> impl ServiceFuture<Self::Response, Self::Error> {
        self.service.call((self.f)(req))
    }
}

impl<S, F> MapServiceRequest<S, F> {
    pub fn new(service: S, f: F) -> Self {
        Self { service, f }
    }
}

impl<S, T> MakeService<T, http::Request<hyper::body::Incoming>> for S
where
    S: Service<http::Request<hyper::body::Incoming>, Error: Error + Send + Sync + 'static>
        + Clone
        + Send
        + 'static,
{
    type Service = S;

    #[inline]
    fn make_service(&self, _target: T) -> Self::Service {
        self.clone()
    }
}
