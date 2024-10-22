use crate::{
    body::BodyError,
    service::{Service, ServiceFuture},
    Layer, Request,
};

#[must_use]
pub struct LimitReqBody<S = ()> {
    inner: S,
    limit: u64,
    reject: bool,
}

impl<S: Default> Default for LimitReqBody<S> {
    fn default() -> Self {
        Self {
            inner: S::default(),
            limit: u64::MAX,
            reject: true,
        }
    }
}

impl LimitReqBody {
    /// Create a new `LimitReqBody` layer with the specified limit, with
    /// the request being rejected if the body size is known to exceed the limit.
    ///
    /// This behavior can be changed with the [`reject`](Self::reject) method.
    pub const fn new(limit: u64) -> Self {
        Self {
            inner: (),
            limit,
            reject: true,
        }
    }

    /// In addition to limiting the request body size, reject the request if the body size
    /// is known to exceed the limit. This is useful for rejecting requests early if the
    /// body size is known to be too large.
    ///
    /// Default to `true`
    pub const fn reject(mut self, reject: bool) -> Self {
        self.reject = reject;
        self
    }
}

impl<S> Layer<S> for LimitReqBody {
    type Service = LimitReqBody<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LimitReqBody {
            inner,
            limit: self.limit,
            reject: self.reject,
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

            if self.reject && body.original_size_hint().lower() > self.limit {
                return Err(BodyError::LengthLimitError.into());
            }

            match body.limit(self.limit) {
                Ok(body) => self.inner.call(Request::from_parts(parts, body)).await,
                Err(e) => Err(e.into()),
            }
        }
    }
}
