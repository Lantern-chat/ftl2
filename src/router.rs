use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use futures::FutureExt;
use http::{Method, StatusCode};
use matchit::Match;

use crate::handler::{BoxedErasedHandler, Handler};

type InnerRouter = matchit::Router<NodeId>;

type NodeId = u64;

pub struct Route<S> {
    path: Arc<str>,
    handler: BoxedErasedHandler<S>,
}

pub struct Router<S> {
    get: InnerRouter,
    post: InnerRouter,
    put: InnerRouter,
    delete: InnerRouter,
    patch: InnerRouter,
    head: InnerRouter,
    connect: InnerRouter,
    options: InnerRouter,
    trace: InnerRouter,
    any: InnerRouter,
    routes: HashMap<NodeId, Route<S>, rustc_hash::FxRandomState>,
    fallback: Option<BoxedErasedHandler<S>>,
    state: S,
    counter: u64,
}

macro_rules! impl_add_route {
    ($($method:ident,)*) => {$(
        pub fn $method<P, H, T>(&mut self, path: P, handler: H) -> &mut Self
        where
            H: Handler<T, S> + Send + 'static,
            T: Send + 'static,
            S: Clone + Send + Sync + 'static,
            P: AsRef<str> + Into<String>,
        {
            assert!(path.as_ref().starts_with("/"), "path must start with /");

            let id = self.counter;
            self.counter += 1;
            self.routes.insert(id, Route {
                path: Arc::from(path.as_ref()),
                handler: BoxedErasedHandler::erase(handler),
            });
            self.$method.insert(path, id).unwrap();
            self
        }
    )*};
}

impl<S> Router<S> {
    pub fn with_state(state: S) -> Self {
        Router {
            get: InnerRouter::new(),
            post: InnerRouter::new(),
            put: InnerRouter::new(),
            delete: InnerRouter::new(),
            patch: InnerRouter::new(),
            head: InnerRouter::new(),
            connect: InnerRouter::new(),
            options: InnerRouter::new(),
            trace: InnerRouter::new(),
            any: InnerRouter::new(),
            fallback: None,
            routes: HashMap::default(),
            state,
            counter: 0,
        }
    }

    pub(crate) fn match_route<'p>(
        &self,
        method: &Method,
        path: &'p str,
    ) -> Result<matchit::Match<'_, 'p, &Route<S>>, Option<&BoxedErasedHandler<S>>> {
        let mut any = false;

        let router = match *method {
            Method::GET => &self.get,
            Method::POST => &self.post,
            Method::PUT => &self.put,
            Method::DELETE => &self.delete,
            Method::PATCH => &self.patch,
            Method::HEAD => &self.head,
            Method::CONNECT => &self.connect,
            Method::OPTIONS => &self.options,
            Method::TRACE => &self.trace,
            _ => {
                any = true;
                &self.any
            }
        };

        let res = match router.at(path) {
            Ok(match_) => Some(match_),
            // fallback to any if not already matching on any
            Err(_) if !any => self.any.at(path).ok(),
            _ => None,
        };

        match res {
            Some(match_) => {
                let handler = match self.routes.get(match_.value) {
                    Some(handler) => handler,
                    None => return Err(self.fallback.as_ref()),
                };

                Ok(Match {
                    value: handler,
                    params: match_.params,
                })
            }
            None => Err(self.fallback.as_ref()),
        }
    }

    impl_add_route! {
        get,
        post,
        put,
        delete,
        patch,
        head,
        connect,
        options,
        trace,
        any,
    }
}

use crate::response::IntoResponse;
use crate::{service::Service, Request, Response};

impl<S> Service<Request> for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    type Response = Response;
    type Error = Infallible;

    fn call(
        &self,
        req: Request,
    ) -> impl Future<Output = Result<Self::Response, Self::Error>> + Send + 'static {
        let (mut parts, body) = req.into_parts();

        let handler = match self.match_route(&parts.method, parts.uri.path()) {
            Ok(match_) => {
                crate::params::insert_url_params(&mut parts.extensions, match_.params);

                parts
                    .extensions
                    .insert(crate::extract::MatchedPath(match_.value.path.clone()));

                &match_.value.handler
            }
            Err(Some(fallback)) => fallback,
            // NotFound is a ZST, so it won't allocate when boxed
            Err(None) => return NotFound.boxed().map(Ok),
        };

        let req = Request::from_parts(parts, body);
        handler.call(req, self.state.clone()).map(Ok)
    }
}

struct NotFound;

use std::pin::Pin;
use std::task::{Context, Poll};

impl Future for NotFound {
    type Output = Response;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(StatusCode::NOT_FOUND.into_response())
    }
}
