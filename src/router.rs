use std::collections::HashMap;
use std::convert::Infallible;
use std::marker::PhantomData;
use std::sync::Arc;

use futures::FutureExt as _;
use http::Method;

use tower_layer::Layer;

use crate::{
    extract::MatchedPath,
    handler::{BoxedErasedHandler, Handler, HandlerIntoResponse},
    service::{Service, ServiceFuture},
    IntoResponse, Request, Response,
};

type NodeId = u64;

pub struct HandlerService<STATE, RETURN> {
    state: STATE,
    handler: BoxedErasedHandler<STATE, RETURN>,
}

impl<STATE, RETURN> Clone for HandlerService<STATE, RETURN>
where
    STATE: Clone,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            handler: self.handler.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Route<SERVICE> {
    path: Arc<str>,
    service: SERVICE,
}

impl<S> Route<S> {
    fn wrap<L>(self, layer: &L) -> Route<L::Service>
    where
        L: Layer<S>,
    {
        Route {
            path: self.path,
            service: layer.layer(self.service),
        }
    }
}

pub trait FromHandler<STATE, T, H> {
    fn from_handler(handler: H, state: STATE) -> Self;
}

impl<STATE, T, H, R> FromHandler<STATE, T, H> for HandlerService<STATE, R>
where
    H: Handler<T, STATE, Output = R>,
    STATE: Clone + Send + Sync + 'static,
    R: 'static,
{
    fn from_handler(handler: H, state: STATE) -> Self {
        HandlerService {
            state,
            handler: BoxedErasedHandler::erase(handler),
        }
    }
}

pub struct Router<STATE, RETURN = Response, SERVICE = HandlerService<STATE, RETURN>> {
    r_get: matchit::Router<NodeId>,
    r_post: matchit::Router<NodeId>,
    r_put: matchit::Router<NodeId>,
    r_delete: matchit::Router<NodeId>,
    r_patch: matchit::Router<NodeId>,
    r_head: matchit::Router<NodeId>,
    r_connect: matchit::Router<NodeId>,
    r_options: matchit::Router<NodeId>,
    r_trace: matchit::Router<NodeId>,
    r_any: matchit::Router<NodeId>,
    routes: HashMap<NodeId, Route<SERVICE>, rustc_hash::FxRandomState>,
    state: STATE,
    counter: u64,
    trim_trailing_slash: bool,
    _return: PhantomData<fn() -> RETURN>,
}

impl<STATE, RETURN, SERVICE> Router<STATE, RETURN, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
    RETURN: 'static,
{
    pub fn with_state(state: STATE) -> Self {
        Router {
            r_get: matchit::Router::new(),
            r_post: matchit::Router::new(),
            r_put: matchit::Router::new(),
            r_delete: matchit::Router::new(),
            r_patch: matchit::Router::new(),
            r_head: matchit::Router::new(),
            r_connect: matchit::Router::new(),
            r_options: matchit::Router::new(),
            r_trace: matchit::Router::new(),
            r_any: matchit::Router::new(),
            routes: HashMap::default(),
            state,
            counter: 1,
            trim_trailing_slash: true,
            _return: PhantomData,
        }
    }

    pub fn state(&self) -> &STATE {
        &self.state
    }

    pub fn trim_trailing_slash(mut self, trim: bool) -> Self {
        self.trim_trailing_slash = trim;
        self
    }

    pub fn route_layer<L>(self, layer: L) -> Router<STATE, RETURN, L::Service>
    where
        L: Layer<SERVICE>,
    {
        Router {
            routes: self.routes.into_iter().map(|(id, route)| (id, route.wrap(&layer))).collect(),

            r_get: self.r_get,
            r_post: self.r_post,
            r_put: self.r_put,
            r_delete: self.r_delete,
            r_patch: self.r_patch,
            r_head: self.r_head,
            r_connect: self.r_connect,
            r_options: self.r_options,
            r_trace: self.r_trace,
            r_any: self.r_any,
            state: self.state,
            counter: self.counter,
            trim_trailing_slash: self.trim_trailing_slash,
            _return: PhantomData,
        }
    }

    pub(crate) fn _on(&mut self, path: &str, methods: &[Method], service: SERVICE) {
        let id = self.counter;
        self.counter += 1;
        self.routes.insert(
            id,
            Route {
                path: Arc::from(path),
                service,
            },
        );

        for method in methods {
            let router = match *method {
                Method::GET => &mut self.r_get,
                Method::POST => &mut self.r_post,
                Method::PUT => &mut self.r_put,
                Method::DELETE => &mut self.r_delete,
                Method::PATCH => &mut self.r_patch,
                Method::HEAD => &mut self.r_head,
                Method::CONNECT => &mut self.r_connect,
                Method::OPTIONS => &mut self.r_options,
                Method::TRACE => &mut self.r_trace,
                _ => &mut self.r_any,
            };

            router.insert(path, id).unwrap();
        }
    }
}

macro_rules! impl_add_route {
    (@INTO_RESPONSE $($method:ident => $upper:ident,)*) => {$(
        pub fn $method<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
        where
            H: Handler<T, STATE, Output: IntoResponse>,
            SERVICE: FromHandler<STATE, T, HandlerIntoResponse<H>>,
        {
            self.on(
                &[Method::$upper],
                path,
                handler,
            )
        }
    )*};

    (@GENERIC_DECL $($method:ident => $upper:ident,)*) => {$(
        fn $method<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
        where
            H: Handler<T, STATE, Output = RETURN>,
            SERVICE: FromHandler<STATE, T, H>;
    )*};

    (@GENERIC_IMPL $($method:ident => $upper:ident,)*) => {$(
        fn $method<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
        where
            H: Handler<T, STATE, Output = RETURN>,
            SERVICE: FromHandler<STATE, T, H>,
        {
            self.on(
                &[Method::$upper],
                path,
                handler,
            )
        }
    )*};
}

impl<STATE, SERVICE> Router<STATE, Response, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
{
    pub fn any<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output: IntoResponse>,
        SERVICE: FromHandler<STATE, T, HandlerIntoResponse<H>>,
    {
        GenericRouter::any(self, path, HandlerIntoResponse(handler))
    }

    pub fn fallback<H, T>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output: IntoResponse>,
        SERVICE: FromHandler<STATE, T, HandlerIntoResponse<H>>,
    {
        GenericRouter::fallback(self, HandlerIntoResponse(handler))
    }

    pub fn ws<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output: IntoResponse>,
        SERVICE: FromHandler<STATE, T, HandlerIntoResponse<H>>,
    {
        GenericRouter::ws(self, path, HandlerIntoResponse(handler))
    }

    pub fn on<H, T>(&mut self, methods: impl AsRef<[Method]>, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output: IntoResponse>,
        SERVICE: FromHandler<STATE, T, HandlerIntoResponse<H>>,
    {
        GenericRouter::on(self, methods, path, HandlerIntoResponse(handler))
    }

    impl_add_route! {@INTO_RESPONSE
        get => GET,
        post => POST,
        put => PUT,
        delete => DELETE,
        patch => PATCH,
        head => HEAD,
        connect => CONNECT,
        options => OPTIONS,
        trace => TRACE,
    }
}

pub trait GenericRouter<STATE, RETURN, SERVICE> {
    fn any<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>;

    fn fallback<H, T>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>;

    fn ws<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>;

    fn on<H, T>(&mut self, methods: impl AsRef<[Method]>, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>;

    impl_add_route! {@GENERIC_DECL
        get => GET,
        post => POST,
        put => PUT,
        delete => DELETE,
        patch => PATCH,
        head => HEAD,
        connect => CONNECT,
        options => OPTIONS,
        trace => TRACE,
    }
}

impl<STATE, RETURN, SERVICE> GenericRouter<STATE, RETURN, SERVICE> for Router<STATE, RETURN, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
    RETURN: 'static,
{
    fn any<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>,
    {
        let path = path.as_ref();

        assert!(path.starts_with("/"), "path must start with /");

        let id = self.counter;
        self.counter += 1;
        self.routes.insert(
            id,
            Route {
                path: Arc::from(path),
                service: SERVICE::from_handler(handler, self.state.clone()),
            },
        );

        self.r_any.insert(path, id).unwrap();

        self
    }

    fn fallback<H, T>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>,
    {
        self.routes.insert(
            0,
            Route {
                path: Arc::default(),
                service: SERVICE::from_handler(handler, self.state.clone()),
            },
        );

        self
    }

    /// Routes specific to WebSocket connections.
    fn ws<H, T>(&mut self, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>,
    {
        self.on(&[Method::GET, Method::CONNECT], path, handler)
    }

    fn on<H, T>(&mut self, methods: impl AsRef<[Method]>, path: impl AsRef<str>, handler: H) -> &mut Self
    where
        H: Handler<T, STATE, Output = RETURN>,
        SERVICE: FromHandler<STATE, T, H>,
    {
        let path = path.as_ref();

        assert!(path.starts_with("/"), "path must start with /");

        self._on(
            path,
            methods.as_ref(),
            SERVICE::from_handler(handler, self.state.clone()),
        );

        self
    }

    impl_add_route! {@GENERIC_IMPL
        get => GET,
        post => POST,
        put => PUT,
        delete => DELETE,
        patch => PATCH,
        head => HEAD,
        connect => CONNECT,
        options => OPTIONS,
        trace => TRACE,
    }
}

impl<STATE, RETURN, SERVICE> Router<STATE, RETURN, SERVICE> {
    pub(crate) fn match_route<'p>(
        &self,
        method: &Method,
        mut path: &'p str,
    ) -> Result<matchit::Match<'_, 'p, &Route<SERVICE>>, Option<&Route<SERVICE>>> {
        let mut any = false;

        let router = match *method {
            Method::GET => &self.r_get,
            Method::POST => &self.r_post,
            Method::PUT => &self.r_put,
            Method::DELETE => &self.r_delete,
            Method::PATCH => &self.r_patch,
            Method::HEAD => &self.r_head,
            Method::CONNECT => &self.r_connect,
            Method::OPTIONS => &self.r_options,
            Method::TRACE => &self.r_trace,
            _ => {
                any = true;
                &self.r_any
            }
        };

        if self.trim_trailing_slash {
            path = path.trim_end_matches('/');
        }

        let maybe_match = match router.at(path) {
            Ok(match_) => Some(match_),
            // fallback to any if not already matching on any
            Err(_) if !any => self.r_any.at(path).ok(),
            _ => None,
        };

        match maybe_match {
            Some(match_) => {
                let handler = match self.routes.get(match_.value) {
                    Some(handler) => handler,
                    None => return Err(self.routes.get(&0)),
                };

                Ok(matchit::Match {
                    value: handler,
                    params: match_.params,
                })
            }
            None => Err(self.routes.get(&0)), // fallback route
        }
    }
}

impl<STATE, RETURN, SERVICE> Router<STATE, RETURN, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
    SERVICE: Service<Request, Response = RETURN, Error = Infallible> + 'static,
    RETURN: Send + 'static,
{
    pub fn finish<B>(self) -> impl Service<http::Request<B>, Response = RETURN, Error = crate::Error>
    where
        B: http_body::Body<Data = bytes::Bytes, Error: std::error::Error + Send + Sync + 'static> + Send + 'static,
    {
        self.route_layer(crate::layers::convert_body::ConvertBody::default())
    }
}

impl<STATE, RETURN, SERVICE> Router<STATE, RETURN, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
    RETURN: Send + 'static,
{
    pub async fn call_opt<B>(&self, req: http::Request<B>) -> Result<Option<RETURN>, SERVICE::Error>
    where
        SERVICE: Service<http::Request<B>, Response = RETURN> + 'static,
        B: Send,
    {
        let (mut parts, body) = req.into_parts();

        let route = match self.match_route(&parts.method, parts.uri.path()) {
            Ok(match_) => {
                crate::params::insert_url_params(&mut parts.extensions, match_.params);

                parts.extensions.insert(MatchedPath(match_.value.path.clone()));

                match_.value
            }
            Err(Some(fallback)) => fallback,
            Err(None) => return Ok(None),
        };

        match route.service.call(http::Request::from_parts(parts, body)).await {
            Ok(res) => Ok(Some(res)),
            Err(err) => Err(err),
        }
    }
}

impl<STATE, RETURN, SERVICE, B> Service<http::Request<B>> for Router<STATE, RETURN, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
    SERVICE: Service<http::Request<B>, Response = RETURN, Error = Infallible> + 'static,
    RETURN: Send + 'static,
    B: Send,
{
    type Response = RETURN;
    type Error = crate::Error;

    #[inline]
    fn call(&self, req: http::Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        async move {
            match self.call_opt(req).await {
                Ok(Some(resp)) => Ok(resp),
                Ok(None) => Err(crate::Error::NotFound),
            }
        }
    }
}

impl<STATE, RETURN> Service<Request> for HandlerService<STATE, RETURN>
where
    STATE: Clone + Send + Sync + 'static,
    RETURN: 'static,
{
    type Response = RETURN;
    type Error = Infallible;

    #[inline]
    fn call(&self, req: Request) -> impl ServiceFuture<Self::Response, Self::Error> {
        self.handler.call(req, self.state.clone()).map(Ok)
    }
}
