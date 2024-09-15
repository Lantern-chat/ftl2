use std::{
    any::TypeId,
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    future::Ready,
    hash::Hash,
    ops::Deref,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::response::{IntoResponse, Response};
use crate::{
    extract::{FromRequestParts, MatchedPath as FtlMatchedPath},
    service::ServiceFuture,
};
use crate::{Layer, Service};
use http::{request::Parts, Extensions, Method, Request};

/// Trait for user-provided keys used to identify rate limiter entries.
///
/// Keys should be uniquely identifiable to avoid rate limiting other users,
/// e.g. using a user ID or IP address.
///
/// Keys must also implement [`FromRequestParts`] to extract the key from the request
/// within the rate limiter layer/service.
pub trait Key: Hash + Eq + Send + Sync + 'static {}

impl<K> Key for K where K: Hash + Eq + Send + Sync + 'static {}

pub mod gcra;
pub use gcra::RateLimitError;

/// Interval for garbage collection of the rate limiter, which can be either
/// a number of requests or a time duration.
///
/// The default is 8192 requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GCInterval {
    /// Run garbage collection after a number of requests.
    ///
    /// This may temporarily block a request if the rate limiter is being cleaned,
    /// as that single request needs to wait on all parts of the table to be cleaned.
    ///
    /// Setting this to `u64::MAX` will disable garbage collection entirely.
    Requests(u64),

    /// Run garbage collection on a timed interval using a background task.
    ///
    /// This does not block the request, since it runs externally to the request.
    Time(Duration),
}

impl Default for GCInterval {
    fn default() -> Self {
        GCInterval::Requests(8192)
    }
}

impl GCInterval {
    fn to_requests(self) -> u64 {
        match self {
            GCInterval::Requests(n) => n,
            GCInterval::Time(_) => u64::MAX,
        }
    }
}

impl From<u64> for GCInterval {
    fn from(n: u64) -> Self {
        GCInterval::Requests(n)
    }
}

impl From<Duration> for GCInterval {
    fn from(d: Duration) -> Self {
        GCInterval::Time(d)
    }
}

/// A route for rate limiting, consisting of a path and method.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Route<'a> {
    pub method: Cow<'a, Method>,
    pub path: Cow<'a, str>,
}

impl<'a, P> From<(Method, P)> for Route<'a>
where
    P: Into<Cow<'a, str>>,
{
    fn from((method, path): (Method, P)) -> Self {
        Self {
            path: path.into(),
            method: Cow::Owned(method),
        }
    }
}

macro_rules! decl_route_methods {
    ($($fn:ident => $method:ident),*) => {
        impl<'a> Route<'a> {
            /// Create a new route with the given method and path.
            pub fn new(method: Method, path: impl Into<Cow<'a, str>>) -> Self {
                Route {
                    method: Cow::Owned(method),
                    path: path.into(),
                }
            }

            $(
                #[doc = concat!("Create a new route with the [`", stringify!($method), "`](Method::", stringify!($method), ") method.")]
                pub fn $fn(path: impl Into<Cow<'a, str>>) -> Route<'a> {
                    Route::new(Method::$method, path)
                }
            )*
        }
    };
}

decl_route_methods! {
    get     => GET,
    post    => POST,
    put     => PUT,
    delete  => DELETE,
    patch   => PATCH,
    options => OPTIONS,
    head    => HEAD,
    trace   => TRACE,
    connect => CONNECT
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RouteWithKey<T> {
    path: MatchedPath,
    method: Method,
    key: T,
}

impl<T> RouteWithKey<T> {
    #[inline]
    fn as_route(&self) -> Route {
        Route {
            path: Cow::Borrowed(&*self.path),
            method: Cow::Borrowed(&self.method),
        }
    }
}

/// Hashmap of quotas for rate limiting, mapping a path as passed to [`Router`](crate::router::Router) to a [`gcra::Quota`].
type Quotas = HashMap<Route<'static>, gcra::Quota, rustc_hash::FxRandomState>;

#[derive(Debug, Clone)]
enum MatchedPath {
    Fallback,
    Matched(FtlMatchedPath),
}

impl Deref for MatchedPath {
    type Target = str;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        match self {
            MatchedPath::Fallback => "",
            MatchedPath::Matched(path) => path,
        }
    }
}

impl PartialEq for MatchedPath {
    fn eq(&self, other: &Self) -> bool {
        let a = &**self;
        let b = &**other;

        // compare Arc pointers first
        a.as_ptr() == b.as_ptr() || a == b
    }
}

impl Eq for MatchedPath {}

impl Hash for MatchedPath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

/// Rate limiter [`Service`] for FTL.
///
/// This struct is not meant to be used directly, but rather through the [`RateLimitLayerBuilder`].
///
/// Note: The limiter is shared across all clones of the layer and service.
pub struct RateLimitService<I, K: Key = ()> {
    inner: I,
    layer: RateLimitLayer<K>,
}

#[derive(Default, Clone)]
struct BuilderDropNotify {
    notify: Arc<tokio::sync::Notify>,
}

/// Builder for the rate limiter layer.
///
/// This struct is used to configure the rate limiter before building it.
pub struct RateLimitLayerBuilder<K = ()> {
    quotas: Quotas,
    default_quota: gcra::Quota,
    set_ext: Option<Box<dyn SetExtension<K>>>,
    global_fallback: bool,
    gc_interval: GCInterval,
    shutdown: BuilderDropNotify,
}

impl<K> Drop for RateLimitLayerBuilder<K> {
    fn drop(&mut self) {
        self.shutdown.notify.notify_waiters();
    }
}

/// Rate limiter [`Layer`] for FTL.
///
/// This struct is not meant to be used directly, but rather through the [`RateLimitLayerBuilder`].
///
/// Note: The limiter is shared across all clones of the layer and service.
pub struct RateLimitLayer<K: Key = ()> {
    builder: Arc<RateLimitLayerBuilder<K>>,
    limiter: Arc<gcra::RateLimiter<RouteWithKey<K>, rustc_hash::FxRandomState>>,
}

/// Object-safe trait for setting an extension on a request.
///
/// Used to insert the rate limiter into the request's extensions,
/// without knowing the type of the key except when the handler is defined and not further.
trait SetExtension<K: Key>: Send + Sync + 'static {
    fn set_extension(&self, req: &mut Extensions, key: &RouteWithKey<K>, layer: RateLimitLayer<K>);
}

struct DoSetExtension;

impl<K: Key> SetExtension<K> for DoSetExtension
where
    K: Clone,
{
    fn set_extension(&self, req: &mut Extensions, key: &RouteWithKey<K>, layer: RateLimitLayer<K>) {
        req.insert(extensions::RateLimiter {
            key: key.clone(),
            layer,
        });
    }
}

impl<K: Key> Clone for RateLimitLayer<K> {
    fn clone(&self) -> Self {
        Self {
            limiter: self.limiter.clone(),
            builder: self.builder.clone(),
        }
    }
}

impl<I: Clone, K: Key> Clone for RateLimitService<I, K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            layer: self.layer.clone(),
        }
    }
}

impl<K: Key> RateLimitLayer<K> {
    /// Begin building a new rate limiter layer starting with the default configuration.
    #[must_use]
    pub fn builder() -> RateLimitLayerBuilder<K> {
        RateLimitLayerBuilder::new()
    }
}

impl<K: Key> RateLimitLayerBuilder<K> {
    #[must_use]
    pub fn new() -> Self {
        RateLimitLayerBuilder {
            quotas: Default::default(),
            default_quota: Default::default(),
            set_ext: None,
            global_fallback: false,
            gc_interval: GCInterval::default(),
            shutdown: BuilderDropNotify::default(),
        }
    }

    /// Insert a route entry into the quota table for the rate limiter.
    pub fn add_route(&mut self, route: impl Into<Route<'static>>, quota: gcra::Quota) {
        self.add_routes(Some((route.into(), quota)));
    }

    /// Insert a route entry into the quota table for the rate limiter.
    #[must_use]
    pub fn with_route(mut self, route: impl Into<Route<'static>>, quota: gcra::Quota) -> Self {
        self.add_route(route.into(), quota);
        self
    }

    /// Insert many route entries into the quota table for the rate limiter.
    pub fn add_routes(&mut self, quotas: impl IntoIterator<Item = (impl Into<Route<'static>>, gcra::Quota)>) {
        self.quotas.extend(quotas.into_iter().map(|(route, quota)| (route.into(), quota)));
    }

    /// Insert many route entries into the quota table for the rate limiter.
    #[must_use]
    pub fn with_routes(
        mut self,
        quotas: impl IntoIterator<Item = (impl Into<Route<'static>>, gcra::Quota)>,
    ) -> Self {
        self.add_routes(quotas);
        self
    }

    /// Fallback quota for rate limiting if no specific quota is found for the path.
    #[must_use]
    pub fn with_default_quota(mut self, default_quota: gcra::Quota) -> Self {
        self.default_quota = default_quota;
        self
    }

    /// Set whether to use a global fallback shared rate-limiter for all paths not explicitly defined.
    #[must_use]
    pub fn with_global_fallback(mut self, global_fallback: bool) -> Self {
        self.global_fallback = global_fallback;
        self
    }

    /// Set the interval for which garbage collection for the rate limiter will occur.
    /// Garbage collection in this case is defined as removing old expired requests
    /// from the rate limiter table to avoid it growing indefinitely.
    ///
    /// The default is 8192 requests.
    ///
    /// This can also be a time [`Duration`],
    /// and a background task will be spawned to clean the rate limiter at the
    /// given time interval. Cleanup is asynchronous and will not block the request
    /// in this case.
    #[must_use]
    pub fn with_gc_interval(mut self, gc_interval: impl Into<GCInterval>) -> Self {
        self.gc_interval = gc_interval.into();
        self
    }

    /// Set whether to insert the [`RateLimiter`](extensions::RateLimiter) extension into the request
    /// to allow for manual rate limiting control downstream.
    ///
    /// # Example
    ///
    /// ```rust,no_run,ignore
    /// # use std::time::Duration;
    /// use ftl::{extract::Extension, routing::get, Router};
    /// use axum_gcra::{RateLimitLayer, extensions::RateLimiter};
    ///
    /// // Note this must be identical to the key used in the rate limiter layer
    /// type Key = ();
    ///
    /// let app = Router::<()>::new()
    ///     // access the rate limiter for this request
    ///     .route("/", get(|rl: Extension<RateLimiter<Key>>| async move {
    ///         rl.penalize(Duration::from_secs(50)).await;
    ///     }))
    ///     .route_layer(RateLimitLayer::<Key>::builder().with_extension(true).default_handle_error());
    #[must_use]
    pub fn with_extension(mut self, extend: bool) -> Self
    where
        K: Clone,
    {
        self.set_ext = match extend {
            true => Some(Box::new(DoSetExtension) as Box<dyn SetExtension<K>>),
            false => None,
        };
        self
    }
}

impl Default for RateLimitLayerBuilder<()> {
    fn default() -> Self {
        RateLimitLayerBuilder::new()
    }
}

/// Error wrapper for rate limiting errors or inner service errors.
#[derive(Debug)]
pub enum Error<Inner, Rejection> {
    /// Inner service error.
    ///
    /// For most FTL services, this will be a [`Infallible`].
    Inner(Inner),

    /// Rate limiting error.
    ///
    /// This error is returned when the rate limiter has blocked the request,
    /// and will be passed to the [error handler](RateLimitLayerBuilder::handle_error).
    RateLimit(RateLimitError),

    /// Key extraction rejection.
    KeyRejection(Rejection),
}

impl<Inner, Rejection> IntoResponse for Error<Inner, Rejection>
where
    Inner: IntoResponse,
    Rejection: IntoResponse,
{
    fn into_response(self) -> Response {
        match self {
            Error::RateLimit(e) => e.into_response(),
            Error::KeyRejection(e) => e.into_response(),
            Error::Inner(e) => e.into_response(),
        }
    }
}

impl<K: Key> RateLimitLayer<K> {
    async fn req_peek_key<F>(
        &self,
        mut key: RouteWithKey<K>,
        now: std::time::Instant,
        peek: F,
    ) -> Result<(), RateLimitError>
    where
        F: FnOnce(&RouteWithKey<K>),
    {
        let quota = match self.builder.quotas.get(&key.as_route()).copied() {
            Some(quota) => quota,
            None => {
                if self.builder.global_fallback {
                    key.path = MatchedPath::Fallback;
                }

                self.builder.default_quota
            }
        };

        self.limiter.req_peek_key(key, quota, now, peek).await
    }
}

async fn get_user_key<K>(parts: &mut Parts) -> Result<K, K::Rejection>
where
    K: Key + FromRequestParts<()>,
{
    use core::mem::{size_of, transmute_copy};

    #[inline(always)]
    fn same_ty<A: 'static, B: 'static>() -> bool {
        let b = TypeId::of::<B>();

        // check same type or 1-tuple of the same layout
        TypeId::of::<A>() == b || (TypeId::of::<(A,)>() == b && size_of::<A>() == size_of::<B>())
    }

    // poor man's specialization

    if same_ty::<K, ()>() {
        return Ok(unsafe { transmute_copy::<_, K>(&()) });
    }

    use crate::extract::real_ip::{self, RealIp, RealIpPrivacyMask};

    if same_ty::<K, RealIp>() {
        #[rustfmt::skip]
        let ip = parts.extensions.get::<real_ip::RealIp>().copied()
            .or_else(|| real_ip::get_ip_from_parts(parts));

        if let Some(ip) = ip {
            return Ok(unsafe { transmute_copy::<_, K>(&ip) });
        }
    }

    if same_ty::<K, RealIpPrivacyMask>() {
        #[rustfmt::skip]
        let ip = parts.extensions.get::<real_ip::RealIp>().copied()
            .or_else(|| real_ip::get_ip_from_parts(parts));

        if let Some(ip) = ip {
            return Ok(unsafe { transmute_copy::<_, K>(&real_ip::RealIpPrivacyMask::from(ip)) });
        }
    }

    K::from_request_parts(parts, &()).await
}

impl<I, K, B> Service<Request<B>> for RateLimitService<I, K>
where
    I: Service<Request<B>> + Send,
    K: Key + FromRequestParts<()>,
    B: Send + 'static,
{
    type Response = I::Response;
    type Error = Error<I::Error, K::Rejection>;

    fn call(&self, req: Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        // try to get the current time as close as possible to the request
        let now = Instant::now();

        let path = match req.extensions().get::<FtlMatchedPath>() {
            Some(path) => MatchedPath::Matched(path.clone()),
            None => MatchedPath::Fallback,
        };

        let (mut parts, body) = req.into_parts();

        async move {
            let key = RouteWithKey {
                key: get_user_key(&mut parts).await.map_err(Error::KeyRejection)?,
                path,
                method: parts.method.clone(),
            };

            let res = self.layer.req_peek_key(key, now, |key| {
                if let Some(ref set_ext) = self.layer.builder.set_ext {
                    // set_extension will clone the key internally
                    set_ext.set_extension(&mut parts.extensions, key, self.layer.clone());
                }
            });

            if let Err(e) = res.await {
                return Err(Error::RateLimit(e));
            }

            self.inner.call(Request::from_parts(parts, body)).await.map_err(Error::Inner)
        }
    }
}

impl<K, I> Layer<I> for RateLimitLayer<K>
where
    K: Key,
    I: Clone + Send + 'static,
{
    type Service = RateLimitService<I, K>;

    fn layer(&self, inner: I) -> Self::Service {
        RateLimitService {
            inner,
            layer: self.clone(),
        }
    }
}

use tower_layer::Stack;

use super::handle_error::HandleErrorLayer;

impl<K> RateLimitLayerBuilder<K>
where
    K: Key + FromRequestParts<()>,
{
    /// Build the [`RateLimitLayer`].
    ///
    /// This will create a new rate limiter and, if the [GC interval](RateLimitLayerBuilder::with_gc_interval)
    /// is a time [`Duration`], spawn a background task for garbage collection.
    ///
    /// By itself, `RateLimitLayer` cannot be directly inserted into an [`crate::Router`],
    /// as it requires a [`HandleErrorLayer`] to handle rate limiting errors.
    /// Use [`RateLimitLayerBuilder::handle_error`] or [`RateLimitLayerBuilder::default_handle_error`] to create a stack
    /// with the rate limiter layer and the error-handler layer combined.
    #[must_use]
    pub fn build(self) -> RateLimitLayer<K> {
        let limiter = Arc::new(gcra::RateLimiter::new(
            self.gc_interval.to_requests(),
            rustc_hash::FxRandomState::default(),
        ));

        if let GCInterval::Time(d) = self.gc_interval {
            let limiter = limiter.clone();
            let signal = self.shutdown.clone();

            _ = tokio::task::spawn(async move {
                let mut interval = tokio::time::interval(d);
                loop {
                    tokio::select! { biased;
                        _ = signal.notify.notified() => break,
                        _ = interval.tick() => {},
                    }

                    limiter.clean(Instant::now()).await;

                    // also close task if no more references to the limiter
                    if Arc::strong_count(&limiter) == 1 {
                        break;
                    }
                }
            });
        }

        RateLimitLayer {
            limiter,
            builder: Arc::new(self),
        }
    }

    /// Create a new rate limiter layer with the provided error-handler callback.
    ///
    /// Returns a [`Stack`]-ed layer with the rate limiter layer and the error-handler layer combined
    /// that can be directly inserted into an [`crate::Router`].
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use ftl::{Router, http::StatusCode};
    /// use ftl::layers::rate_limit::RateLimitLayer;
    ///
    /// let builder = RateLimitLayer::<()>::builder();
    ///
    /// let app = Router::with_state(()).layer(
    ///    builder.handle_error(|e| async move {
    ///       StatusCode::TOO_MANY_REQUESTS
    ///    }));
    /// ```
    #[must_use]
    pub fn handle_error<F, R>(self, cb: F) -> Stack<RateLimitLayer<K>, HandleErrorLayer<F, ()>>
    where
        F: Fn(Error<Infallible, K::Rejection>) -> R + Clone,
    {
        Stack::new(self.build(), HandleErrorLayer::new(cb))
    }

    /// Create a new rate limiter layer with the default error-handler callback that simply returns the error
    /// as a [`Response`].
    ///
    /// Returns a [`Stack`]-ed layer with the rate limiter layer and the error-handler layer combined
    /// that can be directly inserted into an [`crate::Router`].
    #[must_use]
    pub fn default_handle_error(
        self,
    ) -> Stack<
        RateLimitLayer<K>,
        HandleErrorLayer<impl Fn(Error<Infallible, K::Rejection>) -> Ready<Response> + Clone, ()>,
    >
    where
        K::Rejection: IntoResponse,
    {
        self.handle_error(|e| core::future::ready(e.into_response()))
    }
}

/// Defines the [`RateLimiter`](extensions::RateLimiter) extension for the request's extensions,
/// extractable with [`Extension<RateLimiter<Key>>`](crate::extract::Extension).
pub mod extensions {
    use super::*;

    /// [`Request`] extension to access the internal rate limiter used during that request,
    /// such as to apply a penalty or reset the rate limit.
    ///
    /// Note that the `K: Key` type must be the
    /// exact same as those given to the [`RateLimitLayerBuilder`]/[`RateLimitLayer`].
    pub struct RateLimiter<K: Key = ()> {
        pub(super) key: RouteWithKey<K>,
        pub(super) layer: RateLimitLayer<K>,
    }

    impl<K: Key> Clone for RateLimiter<K>
    where
        K: Clone,
    {
        fn clone(&self) -> Self {
            Self {
                key: self.key.clone(),
                layer: self.layer.clone(),
            }
        }
    }

    impl<K: Key> RateLimiter<K> {
        /// Get the key used to identify the rate limiter entry.
        #[inline(always)]
        pub fn key(&self) -> &K {
            &self.key.key
        }

        /// Get the path of the route that was rate limited.
        pub fn path(&self) -> &str {
            &self.key.path
        }

        /// Get the method of the route that was rate limited.
        pub fn method(&self) -> &Method {
            &self.key.method
        }

        /// Get the quota for the route that was rate limited.
        pub fn quota(&self) -> gcra::Quota {
            self.layer.builder.quotas.get(&self.key.as_route()).copied().expect("no quota found for route")
        }

        /// See [`gcra::RateLimiter::penalize`] for more information.
        pub async fn penalize(&self, penalty: Duration) -> bool {
            self.layer.limiter.penalize(&self.key, penalty).await
        }

        /// See [`gcra::RateLimiter::penalize_sync`] for more information.
        pub fn penalize_sync(&self, penalty: Duration) -> bool {
            self.layer.limiter.penalize_sync(&self.key, penalty)
        }

        /// See [`gcra::RateLimiter::reset`] for more information.
        pub async fn reset(&self) -> bool {
            self.layer.limiter.reset(&self.key).await
        }

        /// See [`gcra::RateLimiter::reset_sync`] for more information.
        pub fn reset_sync(&self) -> bool {
            self.layer.limiter.reset_sync(&self.key)
        }

        /// See [`gcra::RateLimiter::clean`] for more information.
        pub async fn clean(&self, before: Instant) {
            self.layer.limiter.clean(before).await;
        }

        /// See [`gcra::RateLimiter::clean_sync`] for more information.
        pub fn clean_sync(&self, before: Instant) {
            self.layer.limiter.clean_sync(before);
        }
    }
}
