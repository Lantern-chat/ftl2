#[cfg(feature = "tls-rustls")]
pub mod tls_rustls;

pub mod accept;

use futures::{FutureExt, TryFutureExt};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::{
        conn::auto::{Builder, Http1Builder, Http2Builder},
        graceful::GracefulShutdown,
    },
};

use std::{
    fmt,
    future::{poll_fn, Future},
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

use accept::{Accept, DefaultAcceptor};

use crate::{
    error::BoxError,
    response::IntoResponse,
    service::{MakeService, Service},
};

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

        if count == 0 && self.inner().shutdown.is_notified() {
            self.inner().kill.notify_waiters();
        }
    }
}

impl Handle {
    /// Set a timeout for graceful shutdown, after which the server will be forcefully stopped.
    ///
    /// If set to `None`, the server will wait indefinitely for all connections to close. This
    /// is the default behavior.
    pub fn set_shutdown_timeout(&self, timeout: Option<Duration>) {
        *self.0.deadline.lock().unwrap() = timeout;
    }

    /// Initiates a graceful shutdown of the server.
    pub fn shutdown(&self) {
        self.0.shutdown.notify_waiters();
    }

    /// Immediately stops the server, dropping all active connections.
    pub fn kill(&self) {
        self.0.kill.notify_waiters();
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

    async fn wait(&self) {
        if self.0.conn_count.load(Ordering::SeqCst) == 0 {
            return;
        }

        let deadline = self.0.deadline.lock().unwrap().unwrap_or(Duration::MAX);

        tokio::select! {
            biased;
            _ = tokio::time::sleep(deadline) => self.kill(),
            _ = self.kill_notified() => {},
        }
    }
}

/// HTTP server.
pub struct Server<A = DefaultAcceptor> {
    acceptor: A,
    builder: Builder<TokioExecutor>,
    listener: Listener,
    handle: Handle,
}

#[derive(Debug)]
enum Listener {
    Bind(SocketAddr),
    Std(std::net::TcpListener),
}

impl Server {
    /// Create a server that will bind to provided address.
    pub fn bind(addr: SocketAddr) -> Self {
        Self {
            acceptor: DefaultAcceptor,
            builder: Builder::new(TokioExecutor::new()),
            listener: Listener::Bind(addr),
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
    pub async fn serve<M>(self, make_service: M) -> io::Result<()>
    where
        // M "creates" a service under the given client address
        M: MakeService<SocketAddr, http::Request<Incoming>>,
        A: Clone + Accept<TcpStream, M::Service, Stream: 'static>,
        // The acceptor maps `M::Service` to its own service type.
        A::Service: Service<
            http::Request<Incoming>,
            Response: IntoResponse,
            Error: core::error::Error + Send + Sync + 'static,
        >,
    {
        let Self {
            acceptor,
            builder,
            listener,
            handle,
        } = self;

        let builder = Arc::new(builder);

        // bind or use existing connection
        let incoming = match listener {
            Listener::Bind(addr) => TcpListener::bind(addr).await,
            Listener::Std(std_listener) => {
                std_listener.set_nonblocking(true)?;
                TcpListener::from_std(std_listener)
            }
        }?;

        // only acquire this once before the loop
        let mut shutdown = std::pin::pin!(handle.shutdown_notified());

        loop {
            // accept incoming connections, with slight backoff on failure
            let accept_loop = async {
                loop {
                    match incoming.accept().await {
                        Ok(value) => break value,
                        Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                    }
                }
            };

            // stop server loop if shutdown is requested
            let (stream, socket_addr) = tokio::select! {
                biased;
                res = accept_loop => res,
                _ = &mut shutdown => break,
            };

            let service = make_service.make_service(socket_addr);
            let acceptor = acceptor.clone();
            let builder = builder.clone();
            let watcher = handle.watcher();

            tokio::spawn(async move {
                let Ok((stream, service)) = acceptor.accept(stream, service).await else {
                    return;
                };

                let mut conn = std::pin::pin!(builder.serve_connection_with_upgrades(
                    TokioIo::new(stream),
                    hyper::service::service_fn(|mut req| {
                        req.extensions_mut().insert(socket_addr);

                        service.call(req).map(|res| match res {
                            Ok(resp) => Ok(resp.into_response()),
                            Err(e) => Err(e),
                        })
                    }),
                ));

                tokio::select! {
                    biased;
                    res = &mut conn => match res {
                        Ok(_) => {}
                        Err(e) => log::error!("server connection error: {}", e),
                    },
                    _ = watcher.0.shutdown_notified() => {
                        conn.as_mut().graceful_shutdown();

                        tokio::select! {
                            biased;
                            _ = conn => {}
                            _ = watcher.0.kill_notified() => {},
                        }
                    }
                }
            });
        }

        drop(incoming);

        handle.wait().await;

        Ok(())
    }
}
