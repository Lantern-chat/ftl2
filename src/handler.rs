use core::future::{Future, IntoFuture};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::{
    extract::{FromRequest, FromRequestParts},
    IntoResponse, Request, Response,
};

pub trait Handler<T, S>: Clone + Send + Sync + 'static {
    fn call(self, req: Request, state: S) -> impl Future<Output = Response> + Send + 'static;
}

impl<Func, R, Fut, Res, S> Handler<((),), S> for Func
where
    Func: FnOnce() -> R + Clone + Send + Sync + 'static,
    R: IntoFuture<IntoFuture = Fut> + Send,
    Fut: Future<Output = Res> + Send,
    Res: IntoResponse,
    S: Send + Sync + 'static,
{
    fn call(self, _req: Request, _state: S) -> impl Future<Output = Response> + Send + 'static {
        async move { self().await.into_response() }
    }
}

macro_rules! impl_handler {
    ([$($t:ident),*], $last:ident) => {
        // NOTE: The `Z` parameter avoid conflicts, and is not used.
        impl<Func, R, Fut, S, Res, Z, $($t,)* $last> Handler<(Z, $($t,)* $last,), S> for Func
        where
            Func: FnOnce($($t,)* $last) -> R + Clone + Send + Sync + 'static,
            R: IntoFuture<IntoFuture = Fut> + Send,
            Fut: Future<Output = Res> + Send,
            Res: IntoResponse,
            S: Send + Sync + 'static,
            $($t: FromRequestParts<S> + Send,)*
            $last: FromRequest<S, Z> + Send,
        {
            #[allow(non_snake_case)]
            fn call(self, req: Request, state: S) -> impl Future<Output = Response> + Send + 'static {
                async move {
                    #[allow(unused_mut)]
                    let (mut parts, body) = req.into_parts();

                    $(
                        let $t = match $t::from_request_parts(&mut parts, &state).await {
                            Ok(t) => t,
                            Err(rejection) => return rejection.into_response(),
                        };
                    )*

                    let $last = match $last::from_request(Request::from_parts(parts, body), &state).await {
                        Ok(t) => t,
                        Err(rejection) => return rejection.into_response(),
                    };

                    self($($t,)* $last).await.into_response()
                }
            }
        }
    };
}

all_the_tuples!(impl_handler);

mod private {
    // Marker type for `impl<T: IntoResponse> Handler for T`
    #[allow(missing_debug_implementations)]
    pub enum IntoResponseHandler {}
}

impl<T, S> Handler<private::IntoResponseHandler, S> for T
where
    T: IntoResponse + Clone + Send + Sync + 'static,
{
    fn call(self, _req: Request, _state: S) -> impl Future<Output = Response> + Send + 'static {
        std::future::ready(self.into_response())
    }
}

/// A type-erased handler, equivalent to `Arc<dyn Handler<..., S>>`.
pub(crate) struct BoxedErasedHandler<S>(pub Arc<dyn ErasedHandler<S>>);

impl<S> Clone for BoxedErasedHandler<S> {
    fn clone(&self) -> Self {
        BoxedErasedHandler(self.0.clone())
    }
}

/// Effectively identical to `dyn Handler<T, S>`, but without needing to specify
/// the `T` type parameters. Instead of a vtable, we have a single function
/// pointer, which is more efficient.
struct MakeErasedHandler<H, S> {
    handler: H,
    call: fn(H, Request, S) -> BoxFuture<'static, Response>,
}

/// A trait object for `Handler<T, S>`, but without needing to specify the `T`
pub(crate) trait ErasedHandler<S>: Send + Sync + 'static {
    fn call(&self, req: Request, state: S) -> BoxFuture<'static, Response>;
}

impl<H, S> ErasedHandler<S> for MakeErasedHandler<H, S>
where
    H: Clone + Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    fn call(&self, req: Request, state: S) -> BoxFuture<'static, Response> {
        (self.call)(self.handler.clone(), req, state)
    }
}

impl<S> BoxedErasedHandler<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn erase<T>(handler: impl Handler<T, S>) -> Self {
        BoxedErasedHandler(Arc::new(MakeErasedHandler {
            handler,
            call: |handler, req, state| Box::pin(handler.call(req, state)),
        }))
    }

    pub fn call(&self, req: Request, state: S) -> BoxFuture<'static, Response> {
        self.0.call(req, state)
    }
}

#[cfg(test)]
mod tests {
    use crate::extract::State;

    use super::*;

    fn _test_handler() {
        async fn my_handler(State(_): State<u32>) {}

        let _x: BoxedErasedHandler<u32> = BoxedErasedHandler::erase(my_handler);
    }
}
