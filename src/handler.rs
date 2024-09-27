#![allow(dead_code)]

use core::future::{Future, IntoFuture};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::{
    extract::{FromRequest, FromRequestParts},
    IntoResponse, Request, Response,
};

pub trait Handler<T, S>: Clone + Send + Sync + 'static {
    type Output: 'static;

    fn call(self, req: Request, state: S) -> impl Future<Output = Self::Output> + Send + 'static;
}

/// Maps the Handler output to a Response via `IntoResponse`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(transparent)]
pub struct HandlerIntoResponse<H>(pub H);

impl<H, S, T> Handler<T, S> for HandlerIntoResponse<H>
where
    H: Handler<T, S, Output: IntoResponse>,
    S: Send + 'static,
{
    type Output = Response;

    fn call(self, req: Request, state: S) -> impl Future<Output = Self::Output> + Send + 'static {
        // this is a very common code path, so I want the future here to be as optimized as possible

        #[pin_project::pin_project]
        #[repr(transparent)]
        struct ResponseFuture<F>(#[pin] F);

        use std::pin::Pin;
        use std::task::{Context, Poll};

        impl<F: Future<Output: IntoResponse>> Future for ResponseFuture<F> {
            type Output = Response;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                self.project().0.poll(cx).map(IntoResponse::into_response)
            }
        }

        ResponseFuture(self.0.call(req, state))
    }
}

impl<Func, FRes, Fut, S> Handler<((),), S> for Func
where
    Func: FnOnce() -> FRes + Clone + Send + Sync + 'static,
    FRes: IntoFuture<IntoFuture = Fut>,
    Fut: Future<Output: 'static> + Send,
    S: Send + Sync + 'static,
{
    type Output = Fut::Output;

    fn call(self, _req: Request, _state: S) -> impl Future<Output = Self::Output> + Send + 'static {
        async move { self().await }
    }
}

macro_rules! impl_handler {
    ([$($t:ident),*], $last:ident) => {
        // NOTE: The `Z` parameter avoids conflicts, and is not used.
        impl<Func, R, Fut, S, $($t,)* $last, Z> Handler<(Z, $($t,)* $last,), S> for Func
        where
            Func: FnOnce($($t,)* $last) -> R + Clone + Send + Sync + 'static,
            S: Send + Sync + 'static,
            R: IntoFuture<IntoFuture = Fut>,
            Fut: Future<Output: 'static> + Send,
            $($t: FromRequestParts<S>,)*
            $last: FromRequest<S, Z>,
        {
            type Output = Result<Fut::Output, crate::Error>;

            #[allow(non_snake_case)]
            fn call(self, req: Request, state: S) -> impl Future<Output = Self::Output> + Send + 'static {
                async move {
                    #[allow(unused_mut)]
                    let (mut parts, body) = req.into_parts();

                    $(let $t = $t::from_request_parts(&mut parts, &state).await.map_err(Into::into)?;)*

                    let $last = $last::from_request(Request::from_parts(parts, body), &state).await.map_err(Into::into)?;

                    Ok(self($($t,)* $last).await)
                }
            }
        }
    };
}

all_the_tuples!(impl_handler);

#[allow(missing_debug_implementations)]
mod private {
    // Marker type for `impl<T: IntoResponse> Handler for T`
    pub enum IntoResponseHandler {}
}

impl<T, S> Handler<private::IntoResponseHandler, S> for T
where
    T: IntoResponse + Clone + Send + Sync + 'static,
{
    type Output = Response;

    fn call(self, _req: Request, _state: S) -> impl Future<Output = Response> + Send + 'static {
        std::future::ready(self.into_response())
    }
}

/// A type-erased handler, equivalent to `Arc<dyn Handler<..., S, Output = R>>`.
pub(crate) struct BoxedErasedHandler<S, R>(pub Arc<dyn ErasedHandler<S, R>>);

// fn assert_sync<S: Send + Sync>() {}
// const _: () = {
//     assert_sync::<BoxedErasedHandler<u32, Response>>();
// };

impl<S, R> Clone for BoxedErasedHandler<S, R> {
    fn clone(&self) -> Self {
        BoxedErasedHandler(self.0.clone())
    }
}

/// Effectively identical to `dyn Handler<T, S, Output = R>`, but without needing to specify
/// the `T` type parameters. Instead of a vtable, we have a single function
/// pointer, which is more efficient.
struct MakeErasedHandler<H, S, R> {
    handler: H,
    call: fn(H, Request, S) -> BoxFuture<'static, R>,
}

/// A trait object for `Handler<T, S, Output = R>`, but without needing to specify the `T`
pub(crate) trait ErasedHandler<S, R>: Send + Sync + 'static {
    fn call(&self, req: Request, state: S) -> BoxFuture<'static, R>;
}

impl<H, S, R> ErasedHandler<S, R> for MakeErasedHandler<H, S, R>
where
    H: Clone + Send + Sync + 'static,
    S: Send + Sync + 'static,
    R: 'static,
{
    fn call(&self, req: Request, state: S) -> BoxFuture<'static, R> {
        (self.call)(self.handler.clone(), req, state)
    }
}

impl<S, R> BoxedErasedHandler<S, R>
where
    S: Clone + Send + Sync + 'static,
    R: 'static,
{
    pub fn erase<T>(handler: impl Handler<T, S, Output = R>) -> Self {
        BoxedErasedHandler(Arc::new(MakeErasedHandler {
            handler,
            call: |handler, req, state| Box::pin(handler.call(req, state)),
        }))
    }

    pub fn call(&self, req: Request, state: S) -> BoxFuture<'static, R> {
        self.0.call(req, state)
    }
}

#[cfg(test)]
mod tests {
    use crate::extract::State;

    use super::*;

    fn _test_handler() {
        async fn my_handler(State(_): State<u32>) -> String {
            "test".to_string()
        }

        let _x: BoxedErasedHandler<u32, Response> = BoxedErasedHandler::erase(HandlerIntoResponse(my_handler));
    }
}
