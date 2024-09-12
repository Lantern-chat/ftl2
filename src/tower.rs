use std::{
    sync::Mutex,
    task::{Context, Poll},
};

use crate::service::Service as FtlService;
use tower_service::Service as TowerService;

/// Wrapper for a [`tower::Service`](tower_service::Service) to
/// be used as an [`ftl::Service`](FtlService).
///
/// This is accomplished by wrapping it in a [`Mutex`] to use
/// the `&mut self` methods of [`tower::Service`](tower_service::Service).
pub struct TowerServiceAsFtlService<S> {
    pub inner: Mutex<S>,
}

impl<S, R> FtlService<R> for TowerServiceAsFtlService<S>
where
    S: TowerService<R, Future: Send + 'static> + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.lock().unwrap().poll_ready(cx)
    }

    fn call(
        &self,
        req: R,
    ) -> impl std::future::Future<Output = Result<Self::Response, Self::Error>> + Send + 'static
    {
        self.inner.lock().unwrap().call(req)
    }
}

impl<S> TowerServiceAsFtlService<S> {
    pub const fn new(inner: S) -> Self {
        Self {
            inner: Mutex::new(inner),
        }
    }
}
