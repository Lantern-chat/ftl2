use std::collections::HashMap;
use std::convert::Infallible;
use std::error::Error;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock};

use futures::FutureExt;
use http::header;
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use http_body::Body as _;
use matchit::Match;

use tower_layer::Layer;

use crate::body::Body;
use crate::extract::MatchedPath;
use crate::handler::{BoxedErasedHandler, Handler};

type InnerRouter = matchit::Router<NodeId>;

type NodeId = u64;

#[derive(Clone)]
pub struct HandlerService<STATE> {
    state: STATE,
    handler: BoxedErasedHandler<STATE>,
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

pub struct Router<STATE, SERVICE = HandlerService<STATE>> {
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
    routes: HashMap<NodeId, Route<SERVICE>, rustc_hash::FxRandomState>,
    state: STATE,
    counter: u64,
}

macro_rules! impl_add_route {
    ($($method:ident => $upper:ident,)*) => {$(
        pub fn $method<P, H, T>(&mut self, path: P, handler: H) -> &mut Self
        where
            H: Handler<T, STATE> + Send + 'static,
            T: Send + 'static,
            STATE: Clone + Send + Sync + 'static,
            P: AsRef<str>,
        {
            self.on(
                &[Method::$upper],
                path,
                handler,
            )
        }
    )*};
}

impl<STATE> Router<STATE> {
    pub fn with_state(state: STATE) -> Self {
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
            routes: HashMap::default(),
            state,
            counter: 1,
        }
    }

    pub fn fallback<H, T>(&mut self, handler: H)
    where
        H: Handler<T, STATE> + Send + 'static,
        STATE: Clone + Send + Sync + 'static,
        T: Send + 'static,
    {
        self.routes.insert(
            0,
            Route {
                path: Arc::from(""),
                service: HandlerService {
                    state: self.state.clone(),
                    handler: BoxedErasedHandler::erase(handler),
                },
            },
        );
    }

    /// Routes specific to WebSocket connections.
    pub fn ws<P, H, T>(&mut self, path: P, handler: H) -> &mut Self
    where
        H: Handler<T, STATE> + Send + 'static,
        T: Send + 'static,
        STATE: Clone + Send + Sync + 'static,
        P: AsRef<str>,
    {
        self.on(&[Method::GET, Method::CONNECT], path, handler)
    }

    pub fn on<P, H, T>(&mut self, methods: impl AsRef<[Method]>, path: P, handler: H) -> &mut Self
    where
        H: Handler<T, STATE> + Send + 'static,
        T: Send + 'static,
        STATE: Clone + Send + Sync + 'static,
        P: AsRef<str>,
    {
        assert!(path.as_ref().starts_with("/"), "path must start with /");

        let id = self.counter;
        self.counter += 1;
        self.routes.insert(
            id,
            Route {
                path: Arc::from(path.as_ref()),
                service: HandlerService {
                    state: self.state.clone(),
                    handler: BoxedErasedHandler::erase(handler),
                },
            },
        );

        for method in methods.as_ref() {
            let router = match *method {
                Method::GET => &mut self.get,
                Method::POST => &mut self.post,
                Method::PUT => &mut self.put,
                Method::DELETE => &mut self.delete,
                Method::PATCH => &mut self.patch,
                Method::HEAD => &mut self.head,
                Method::CONNECT => &mut self.connect,
                Method::OPTIONS => &mut self.options,
                Method::TRACE => &mut self.trace,
                _ => &mut self.any,
            };

            router.insert(path.as_ref(), id).unwrap();
        }

        self
    }

    impl_add_route! {
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

    pub fn any<P, H, T>(&mut self, path: P, handler: H) -> &mut Self
    where
        H: Handler<T, STATE> + Send + 'static,
        T: Send + 'static,
        STATE: Clone + Send + Sync + 'static,
        P: AsRef<str>,
    {
        static ANY_METHOD: LazyLock<Method> = LazyLock::new(|| Method::from_bytes(b"ANY").unwrap());

        self.on(&[ANY_METHOD.clone()], path, handler)
    }
}

impl<STATE, SERVICE> Router<STATE, SERVICE> {
    pub fn route_layer<L>(self, layer: L) -> Router<STATE, L::Service>
    where
        L: Layer<SERVICE>,
    {
        Router {
            routes: self.routes.into_iter().map(|(id, route)| (id, route.wrap(&layer))).collect(),

            get: self.get,
            post: self.post,
            put: self.put,
            delete: self.delete,
            patch: self.patch,
            head: self.head,
            connect: self.connect,
            options: self.options,
            trace: self.trace,
            any: self.any,
            state: self.state,
            counter: self.counter,
        }
    }

    pub(crate) fn match_route<'p>(
        &self,
        method: &Method,
        path: &'p str,
    ) -> Result<matchit::Match<'_, 'p, &Route<SERVICE>>, Option<&Route<SERVICE>>> {
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
                    None => return Err(self.routes.get(&0)),
                };

                Ok(Match {
                    value: handler,
                    params: match_.params,
                })
            }
            None => Err(self.routes.get(&0)),
        }
    }
}

use crate::response::IntoResponse;
use crate::service::ServiceFuture;
use crate::{service::Service, Request, Response};

impl<STATE, SERVICE, B> Service<http::Request<B>> for Router<STATE, SERVICE>
where
    STATE: Clone + Send + Sync + 'static,
    SERVICE: Service<Request, Response = Response, Error = Infallible> + 'static,
    B: http_body::Body<Data = bytes::Bytes, Error: Error + Send + Sync + 'static> + Send + 'static,
{
    type Response = Response;
    type Error = Infallible;

    fn call(&self, req: http::Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        use futures::future::Either;

        let (mut parts, body) = req.into_parts();

        let route = match self.match_route(&parts.method, parts.uri.path()) {
            Ok(match_) => {
                crate::params::insert_url_params(&mut parts.extensions, match_.params);

                parts.extensions.insert(MatchedPath(match_.value.path.clone()));

                match_.value
            }
            Err(Some(fallback)) => fallback,
            Err(None) => return Either::Right(NotFound),
        };

        Either::Left(route.service.call(Request::from_parts(parts, Body::from_any_body(body))))
    }
}

impl<STATE> Service<Request> for HandlerService<STATE>
where
    STATE: Clone + Send + Sync + 'static,
{
    type Response = Response;
    type Error = Infallible;

    fn call(&self, req: Request) -> impl ServiceFuture<Self::Response, Self::Error> {
        let method = match *req.method() {
            Method::HEAD => MiniMethod::Head,
            Method::CONNECT => MiniMethod::Connect,
            _ => MiniMethod::Other,
        };

        self.handler.call(req, self.state.clone()).map(move |mut resp| {
            if method == MiniMethod::Connect && resp.status().is_success() {
                // From https://httpwg.org/specs/rfc9110.html#CONNECT:
                // > A server MUST NOT send any Transfer-Encoding or
                // > Content-Length header fields in a 2xx (Successful)
                // > response to CONNECT.
                if resp.headers().contains_key(header::CONTENT_LENGTH)
                    || resp.headers().contains_key(header::TRANSFER_ENCODING)
                    || resp.size_hint().lower() != 0
                {
                    log::error!("response to CONNECT with nonempty body");
                    resp = resp.map(|_| Body::empty());
                }
            } else {
                // make sure to set content-length before removing the body
                set_content_length(resp.size_hint(), resp.headers_mut());
            }

            if method == MiniMethod::Head {
                resp = resp.map(|_| Body::empty());
            }

            Ok(resp)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MiniMethod {
    Head,
    Connect,
    Other,
}

fn set_content_length(size_hint: http_body::SizeHint, headers: &mut HeaderMap) {
    if headers.contains_key(header::CONTENT_LENGTH) {
        return;
    }

    if let Some(size) = size_hint.exact() {
        let header_value = if size == 0 {
            #[allow(clippy::declare_interior_mutable_const)]
            const ZERO: HeaderValue = HeaderValue::from_static("0");

            ZERO
        } else {
            let mut buffer = itoa::Buffer::new();
            HeaderValue::from_str(buffer.format(size)).unwrap()
        };

        headers.insert(header::CONTENT_LENGTH, header_value);
    }
}

struct NotFound;

use std::pin::Pin;
use std::task::{Context, Poll};

impl Future for NotFound {
    type Output = Result<Response, Infallible>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(Ok(StatusCode::NOT_FOUND.into_response()))
    }
}
