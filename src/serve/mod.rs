#[cfg(feature = "tls-rustls")]
pub mod tls_rustls;

#[cfg(feature = "tls-openssl")]
pub mod tls_openssl;

pub mod accept;

use core::error::Error;

use futures::{stream::FusedStream, FutureExt, Stream, StreamExt};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::{Builder, Http1Builder, Http2Builder},
};

use std::{
    future::Future,
    io::{self},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};

use tokio::{
    net::{TcpListener, TcpStream},
    sync::Notify,
};

use accept::{Accept, DefaultAcceptor};

use crate::service::{MakeService, Service};

#[derive(Debug, Default)]
struct NotifyOnce {
    notified: AtomicBool,
    notify: Notify,
}

impl NotifyOnce {
    pub(crate) fn notify_waiters(&self) {
        self.notified.store(true, Ordering::SeqCst);

        self.notify.notify_waiters();
    }

    pub(crate) fn is_notified(&self) -> bool {
        self.notified.load(Ordering::SeqCst)
    }

    pub(crate) async fn notified(&self) {
        let future = self.notify.notified();

        if !self.notified.load(Ordering::SeqCst) {
            future.await;
        }
    }
}

#[derive(Default)]
struct HandleInner {
    conn_count: AtomicUsize,
    shutdown: NotifyOnce,
    kill: Notify,
    deadline: Mutex<Option<Duration>>,
}

#[derive(Clone, Default)]
pub struct Handle(Arc<HandleInner>);

struct Watcher(Handle);

impl Watcher {
    fn inner(&self) -> &HandleInner {
        &self.0 .0
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let count = self.inner().conn_count.fetch_sub(1, Ordering::SeqCst);

        // if count == 1, the new count is 0, so if shutdown is requested
        // we should kill the server ASAP.
        if count == 1 && self.inner().shutdown.is_notified() {
            self.inner().kill.notify_waiters();
        }
    }
}

impl Handle {
    /// Set a timeout for graceful shutdown, after which the server will be forcefully stopped.
    ///
    /// If set to `None`, the server will wait indefinitely for all connections to close. This
    /// is the default behavior.
    pub fn set_shutdown_timeout(&self, timeout: impl Into<Option<Duration>>) {
        *self.0.deadline.lock().unwrap() = timeout.into();
    }

    /// Initiates a graceful shutdown of the server.
    pub fn shutdown(&self) {
        self.0.shutdown.notify_waiters();
    }

    /// Immediately stops the server, dropping all active connections.
    pub fn kill(&self) {
        self.0.kill.notify_waiters();
    }

    pub fn shutdown_on<F>(self, signal: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            signal.await;
            self.shutdown();
        });
    }

    fn shutdown_notified(&self) -> impl Future<Output = ()> + '_ {
        self.0.shutdown.notified()
    }

    fn kill_notified(&self) -> impl Future<Output = ()> + '_ {
        self.0.kill.notified()
    }

    fn watcher(&self) -> Watcher {
        self.0.conn_count.fetch_add(1, Ordering::SeqCst);
        Watcher(self.clone())
    }

    async fn wait_internal(&self) {
        if self.0.conn_count.load(Ordering::SeqCst) == 0 {
            self.kill(); // no connections, kill immediately
            return;
        }

        let deadline = self.0.deadline.lock().unwrap().unwrap_or(Duration::MAX);

        tokio::select! {
            biased;
            _ = self.kill_notified() => {},
            _ = tokio::time::sleep(deadline) => self.kill(),
        }
    }

    pub async fn wait(&self) {
        self.kill_notified().await
    }
}

/// HTTP server.
#[must_use]
pub struct Server<A = DefaultAcceptor> {
    acceptor: A,
    builder: Builder<TokioExecutor>,
    listener: Listener,
    handle: Handle,
}

#[derive(Debug)]
enum Listener {
    Bind(Vec<SocketAddr>),
    Std(std::net::TcpListener),
}

impl Server {
    /// Create a server that will bind to provided address.
    pub fn bind(addr: impl IntoIterator<Item = SocketAddr>) -> Self {
        Self {
            acceptor: DefaultAcceptor,
            builder: Builder::new(TokioExecutor::new()),
            listener: Listener::Bind(addr.into_iter().collect()),
            handle: Handle::default(),
        }
    }

    /// Create a server from existing `std::net::TcpListener`.
    pub fn from_tcp(listener: std::net::TcpListener) -> Self {
        Self {
            acceptor: DefaultAcceptor,
            builder: Builder::new(TokioExecutor::new()),
            listener: Listener::Std(listener),
            handle: Handle::default(),
        }
    }
}

impl<A> Server<A>
where
    A: Clone,
{
    pub fn rebind(&self, addr: impl IntoIterator<Item = SocketAddr>) -> Self {
        Self {
            acceptor: self.acceptor.clone(),
            builder: self.builder.clone(),
            listener: Listener::Bind(addr.into_iter().collect()),
            handle: self.handle.clone(),
        }
    }
}

impl<A> Server<A> {
    /// Overwrite acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> Server<Acceptor> {
        Server {
            acceptor,
            builder: self.builder,
            listener: self.listener,
            handle: self.handle,
        }
    }

    /// Map acceptor.
    pub fn map<Acceptor, F>(self, acceptor: F) -> Server<Acceptor>
    where
        F: FnOnce(A) -> Acceptor,
    {
        Server {
            acceptor: acceptor(self.acceptor),
            builder: self.builder,
            listener: self.listener,
            handle: self.handle,
        }
    }

    /// Returns a reference to the acceptor.
    pub fn get_ref(&self) -> &A {
        &self.acceptor
    }

    /// Returns a mutable reference to the acceptor.
    pub fn get_mut(&mut self) -> &mut A {
        &mut self.acceptor
    }

    /// Returns a mutable reference to the Http builder.
    pub fn http_builder(&mut self) -> &mut Builder<TokioExecutor> {
        &mut self.builder
    }

    pub fn http1(&mut self) -> Http1Builder<TokioExecutor> {
        self.builder.http1()
    }

    pub fn http2(&mut self) -> Http2Builder<TokioExecutor> {
        self.builder.http2()
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

impl<A> Server<A> {
    pub async fn serve<M, B>(self, make_service: M) -> io::Result<()>
    where
        // M "creates" a service under the given client address
        M: MakeService<SocketAddr, http::Request<Incoming>>,
        A: Clone + Accept<TcpStream, M::Service, Stream: 'static>,
        M::Service: 'static,
        // The acceptor maps `M::Service` to its own service type.
        A::Service: 'static
            + Clone
            + Service<http::Request<Incoming>, Response = http::Response<B>, Error: Error + Send + Sync + 'static>,
        // Body requirements
        B: http_body::Body<Data: Send, Error: Error + Send + Sync + 'static> + Send + 'static,
    {
        let Self {
            acceptor,
            builder,
            listener,
            handle,
        } = self;

        let builder = Arc::new(builder);

        #[pin_project::pin_project]
        struct IncomingThrottle {
            #[pin]
            incoming: TcpListener,
            #[pin]
            throttle: Option<tokio::time::Sleep>,
        }

        impl FusedStream for IncomingThrottle {
            fn is_terminated(&self) -> bool {
                // TODO: Change this when errors potentially terminate the stream.
                false
            }
        }

        impl Stream for IncomingThrottle {
            type Item = (TcpStream, SocketAddr);

            fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                let mut this = self.project();

                loop {
                    if let Some(throttle) = this.throttle.as_mut().as_pin_mut() {
                        match throttle.poll(cx) {
                            Poll::Pending => return Poll::Pending,
                            Poll::Ready(_) => this.throttle.set(None),
                        }
                    }

                    match this.incoming.poll_accept(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Ok(value)) => return Poll::Ready(Some(value)),
                        Poll::Ready(Err(_)) => {
                            // TODO: Inspect error and potentially return `None` if it's a fatal error?
                            this.throttle.set(Some(tokio::time::sleep(Duration::from_millis(50))));

                            continue;
                        }
                    }
                }
            }
        }

        #[pin_project::pin_project]
        struct FutureWithAssociatedData<F, T> {
            #[pin]
            future: F,
            data: Option<T>,
        }

        impl<F, T> Future for FutureWithAssociatedData<F, T>
        where
            F: Future,
        {
            type Output = (F::Output, T);

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let this = self.project();

                match this.future.poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(value) => Poll::Ready((value, this.data.take().expect("polled after completion"))),
                }
            }
        }

        // bind or use existing connection, then setup throttling
        let mut incoming = std::pin::pin!(IncomingThrottle {
            incoming: match listener {
                Listener::Bind(addr) => TcpListener::bind(&*addr).await,
                Listener::Std(std_listener) => {
                    std_listener.set_nonblocking(true)?;
                    TcpListener::from_std(std_listener)
                }
            }?,
            throttle: None,
        });

        // use a FuturesUnordered to handle the accept process without allocating an entire task.
        // This may help avert DoS attacks by limiting the number of tasks that can be spawned.
        let mut accepting = std::pin::pin!(futures::stream::FuturesUnordered::new());

        // since this is fused, create the future ahead of time to simplify polling.
        let mut shutdown = std::pin::pin!(handle.shutdown_notified().fuse());

        loop {
            // futures::select! is required over tokio::select! due to the `accepting` stream,
            // which may be empty, but not terminated.
            futures::select_biased! {
                // because this is a biased select, shutdown should be checked first so it won't starve.
                _ = &mut shutdown => break,

                // NOTE: This needs to come before the `accepting.select_next_some()` branch
                // to avoid it polling a `None` and being less efficient.
                res = incoming.next() => match res {
                    // NOTE: This `None` branch is technically unreachable due to the current implementation.
                    // However, I'd rather keep it around for future-proofing.
                    None => break,
                    Some((stream, socket_addr)) => accepting.push(FutureWithAssociatedData {
                        future: acceptor.accept(stream, make_service.make_service(socket_addr)),
                        data: Some((socket_addr, handle.watcher())),
                    }),
                },

                accepted = accepting.select_next_some() => match accepted {
                    (Ok((stream, service)), (socket_addr, watcher)) => {
                        let builder = builder.clone();

                        // spawn new task to handle real HTTP connection
                        tokio::spawn(async move {
                            let mut conn = std::pin::pin!(builder.serve_connection_with_upgrades(
                                TokioIo::new(stream),
                                hyper::service::service_fn(move |mut req| {
                                    req.extensions_mut().insert(socket_addr);

                                    // in practice, this should be a single `Arc` clone,
                                    // and it allows us to make `call` non-'static, reducing
                                    // the number of clones internally.
                                    let service = service.clone();
                                    async move { service.call(req).await }
                                }),
                            ));

                            let mut kill = std::pin::pin!(watcher.0.kill_notified());

                            loop {
                                tokio::select! {
                                    biased;

                                    _ = &mut kill => break,

                                    res = &mut conn => {
                                        if let Err(err) = res {
                                            // honestly, ignore logging hyper errors, so only log if it's not hyper
                                            if let Err(err) = err.downcast::<hyper::Error>() {
                                                log::error!("server error: {err:?}");
                                            }
                                        }

                                        break; // connection has completed
                                    },

                                    _ = watcher.0.shutdown_notified() => {
                                        // tell the connection to shutdown gracefully, then continue
                                        conn.as_mut().graceful_shutdown();

                                        continue;
                                    }
                                }
                            }
                        });
                    },
                    _ => continue,
                },
            }
        }

        handle.wait_internal().await;

        Ok(())
    }
}

use std::path::Path;

#[allow(async_fn_in_trait)]
pub trait TlsConfig: Sized + core::fmt::Debug {
    type Error;
    type DerCert;
    type DerCertChain;

    /// Create config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    async fn from_der(cert: Self::DerCertChain, key: Vec<u8>) -> Result<Self, Self::Error>;

    /// Create config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    async fn from_pem(cert: String, key: String) -> Result<Self, Self::Error>;

    /// Create config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    async fn from_pem_file(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<Self, Self::Error>;

    /// Reload config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    async fn reload_from_der(&self, cert: Self::DerCertChain, key: Vec<u8>) -> Result<(), Self::Error>;

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate chain and key.
    async fn from_pem_chain_file(chain: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<Self, Self::Error>;

    /// Reload config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    async fn reload_from_pem(&self, cert: String, key: String) -> Result<(), Self::Error>;

    /// Reload config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    async fn reload_from_pem_file(&self, cert: impl AsRef<Path>, key: impl AsRef<Path>)
        -> Result<(), Self::Error>;

    /// Reload config from a PEM-formatted certificate chain and key.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    async fn reload_from_pem_chain_file(
        &self,
        chain: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<(), Self::Error>;
}
