use std::{convert::Infallible, fmt, future::Future, marker::PhantomData};

use crate::{
    //extract::FromRequestParts,
    http::Request,
    response::{IntoResponse, Response},
    service::ServiceFuture,
    Layer,
    Service,
};

/// [`Layer`] that applies [`HandleError`] which is a [`Service`] adapter
/// that handles errors by converting them into responses.
///
/// See [module docs](self) for more details on axum's error handling model.
pub struct HandleErrorLayer<F, T> {
    f: F,
    _extractor: PhantomData<fn() -> T>,
}

impl<F, T> HandleErrorLayer<F, T> {
    /// Create a new `HandleErrorLayer`.
    pub fn new(f: F) -> Self {
        Self {
            f,
            _extractor: PhantomData,
        }
    }
}

impl<F, T> Clone for HandleErrorLayer<F, T>
where
    F: Clone,
{
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone(),
            _extractor: PhantomData,
        }
    }
}

impl<F, E> fmt::Debug for HandleErrorLayer<F, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandleErrorLayer")
            .field("f", &format_args!("{}", std::any::type_name::<F>()))
            .finish()
    }
}

impl<S, F, T> Layer<S> for HandleErrorLayer<F, T>
where
    F: Clone,
{
    type Service = HandleError<S, F, T>;

    fn layer(&self, inner: S) -> Self::Service {
        HandleError::new(inner, self.f.clone())
    }
}

/// A [`Service`] adapter that handles errors by converting them into responses.
///
/// See [module docs](self) for more details on axum's error handling model.
pub struct HandleError<S, F, T> {
    inner: S,
    f: F,
    _extractor: PhantomData<fn() -> T>,
}

impl<S, F, T> HandleError<S, F, T> {
    /// Create a new `HandleError`.
    pub fn new(inner: S, f: F) -> Self {
        Self {
            inner,
            f,
            _extractor: PhantomData,
        }
    }
}

impl<S, F, T> Clone for HandleError<S, F, T>
where
    S: Clone,
    F: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            f: self.f.clone(),
            _extractor: PhantomData,
        }
    }
}

impl<S, F, E> fmt::Debug for HandleError<S, F, E>
where
    S: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandleError")
            .field("inner", &self.inner)
            .field("f", &format_args!("{}", std::any::type_name::<F>()))
            .finish()
    }
}

impl<S, F, B, Fut, Res> Service<Request<B>> for HandleError<S, F, ()>
where
    S: Service<Request<B>>,
    S::Response: IntoResponse + Send,
    S::Error: Send,
    F: FnOnce(S::Error) -> Fut + Clone + Send + Sync, // TODO: Revisit this, could be Fn() without Clone
    Fut: Future<Output = Res> + Send,
    Res: IntoResponse,
    B: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;

    fn call(&self, req: Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        async move {
            match self.inner.call(req).await {
                Ok(res) => Ok(res.into_response()),
                Err(err) => Ok((self.f.clone())(err).await.into_response()),
            }
        }
    }
}

// TODO: Revisit with new error handling
//
// #[allow(unused_macros)]
// macro_rules! impl_service {
//     ( $($ty:ident),* $(,)? ) => {
//         impl<S, F, B, Res, Fut, $($ty,)*> Service<Request<B>>
//             for HandleError<S, F, ($($ty,)*)>
//         where
//             S: Service<Request<B>> + Clone,
//             S::Response: IntoResponse + Send,
//             S::Error: Send,
//             F: FnOnce($($ty),*, S::Error) -> Fut + Clone + Send + Sync,
//             Fut: Future<Output = Res> + Send,
//             Res: IntoResponse,
//             $( $ty: FromRequestParts<()>,)*
//             B: Send + 'static,
//         {
//             type Response = Response;
//             type Error = Infallible;

//             #[allow(non_snake_case)]
//             fn call(&self, req: Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
//                 async move {
//                     let (mut parts, body) = req.into_parts();

//                     $(
//                         let $ty = match $ty::from_request_parts(&mut parts, &()).await {
//                             Ok(value) => value,
//                             Err(rejection) => return Ok(rejection.into_response()),
//                         };
//                     )*

//                     let req = Request::from_parts(parts, body);

//                     match self.inner.call(req).await {
//                         Ok(res) => Ok(res.into_response()),
//                         Err(err) => Ok((self.f.clone())($($ty),*, err).await.into_response()),
//                     }
//                 }
//             }
//         }
//     }
// }

// impl_service!(T1);
// impl_service!(T1, T2);
// impl_service!(T1, T2, T3);
// impl_service!(T1, T2, T3, T4);
// impl_service!(T1, T2, T3, T4, T5);
// impl_service!(T1, T2, T3, T4, T5, T6);
// impl_service!(T1, T2, T3, T4, T5, T6, T7);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15);
// impl_service!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16);
