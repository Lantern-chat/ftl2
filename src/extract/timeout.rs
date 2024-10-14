use crate::{
    extract::{FromRequest, FromRequestParts},
    Error, Request, RequestParts,
};
use std::{
    future::Future,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

/// A "factory" type for creating timeouts based on the request parts.
///
/// This can be a constant value or dynamic based on the request.
///
/// Example:
/// ```rust,ignore
/// impl TimeoutFactory for FiveSeconds {
///     fn timeout(_parts: &RequestParts) -> Duration {
///         Duration::from_secs(5)
///     }
/// }
///
/// async fn my_handler(value: Timeout<MyValue, FiveSeconds>) {
///     // `value` will be a wrapped `MyValue` if the request completes within 5 seconds, otherwise
///     // the extraction will be rejected with a `Error::TimedOut`.
/// }
/// ```
pub trait TimeoutFactory: Send + Sync + 'static {
    fn timeout(parts: &RequestParts) -> Duration;
}

pub struct Timeout<T, F> {
    pub value: T,
    factory: PhantomData<fn() -> F>,
}

impl<T, F> Deref for Timeout<T, F> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T, F> DerefMut for Timeout<T, F> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

/// Zero-cost abstraction over `tokio::time::Timeout` that allows for customizing
/// the timeout result handling.
#[allow(clippy::type_complexity)]
#[repr(transparent)]
#[pin_project::pin_project]
struct DoTimeout<I, F, T, E>(#[pin] tokio::time::Timeout<I>, PhantomData<fn() -> (F, T, E)>);

impl<I, F, T, E> Future for DoTimeout<I, F, T, E>
where
    I: Future<Output = Result<T, E>>,
    E: Into<Error>,
{
    type Output = Result<Timeout<T, F>, Error>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(match self.project().0.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(res)) => match res {
                Ok(value) => Ok(Timeout {
                    value,
                    factory: PhantomData,
                }),
                Err(err) => Err(err.into()),
            },
            Poll::Ready(Err(_)) => Err(Error::TimedOut),
        })
    }
}

impl<S, T, F> FromRequest<S> for Timeout<T, F>
where
    T: FromRequest<S, Rejection: Into<Error>>,
    F: TimeoutFactory,
{
    type Rejection = Error;

    #[inline]
    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let (parts, body) = req.into_parts();
        let timeout = F::timeout(&parts);
        let req = Request::from_parts(parts, body);

        DoTimeout(tokio::time::timeout(timeout, T::from_request(req, state)), PhantomData)
    }
}

impl<S, T, F> FromRequestParts<S> for Timeout<T, F>
where
    T: FromRequestParts<S, Rejection: Into<Error>>,
    F: TimeoutFactory,
{
    type Rejection = Error;

    #[inline]
    fn from_request_parts(
        parts: &mut RequestParts,
        state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let timeout = F::timeout(parts);

        DoTimeout(
            tokio::time::timeout(timeout, T::from_request_parts(parts, state)),
            PhantomData,
        )
    }
}
