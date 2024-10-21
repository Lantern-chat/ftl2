use crate::{
    body::BodyError,
    service::{Service, ServiceFuture},
    Layer, Request,
};

#[must_use]
pub struct LimitReqBody<S = ()> {
    inner: S,
    limit: usize,
}

impl<S: Default> Default for LimitReqBody<S> {
    fn default() -> Self {
        Self {
            inner: S::default(),
            limit: usize::MAX,
        }
    }
}

impl LimitReqBody {
    pub const fn new(limit: usize) -> Self {
        Self { inner: (), limit }
    }
}

impl<S> Layer<S> for LimitReqBody {
    type Service = LimitReqBody<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LimitReqBody {
            inner,
            limit: self.limit,
        }
    }
}

impl<S, Res> Service<Request> for LimitReqBody<S>
where
    S: Service<Request, Response = Res, Error: From<BodyError>>,
{
    type Response = Res;
    type Error = S::Error;

    #[inline]
    fn call(&self, req: Request) -> impl ServiceFuture<Self::Response, Self::Error> {
        async move {
            let (parts, body) = req.into_parts();

            match body.limit(self.limit) {
                Ok(body) => self.inner.call(Request::from_parts(parts, body)).await,
                Err(e) => Err(e.into()),
            }
        }
    }
}
