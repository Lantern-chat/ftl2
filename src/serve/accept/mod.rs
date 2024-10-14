use std::{future::Future, io};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};

pub mod limited;

/// An asynchronous function to modify io stream and service.
pub trait Accept<I: Send, S: Send>: Send + Sync + 'static {
    /// IO stream produced by accept.
    type Stream: Send + AsyncRead + AsyncWrite + Unpin;

    /// Service produced by accept.
    type Service: Send;

    /// Process io stream and service asynchronously.
    fn accept(
        &self,
        stream: I,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send;
}

/// A no-op acceptor.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultAcceptor;

/// An acceptor that sets `TCP_NODELAY` on accepted streams.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDelayAcceptor;

impl<I, S> Accept<I, S> for DefaultAcceptor
where
    I: Send + AsyncRead + AsyncWrite + Unpin,
    S: Send,
{
    type Stream = I;
    type Service = S;

    #[inline]
    fn accept(
        &self,
        stream: I,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send {
        std::future::ready(Ok((stream, service)))
    }
}

impl<S: Send> Accept<TcpStream, S> for NoDelayAcceptor {
    type Stream = TcpStream;
    type Service = S;

    #[inline]
    fn accept(
        &self,
        stream: TcpStream,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send {
        std::future::ready(stream.set_nodelay(true).and(Ok((stream, service))))
    }
}

use std::time::Duration;

#[derive(Clone)]
pub struct TimeoutAcceptor<A> {
    acceptor: A,
    timeout: Duration,
}

impl<A> TimeoutAcceptor<A> {
    pub const fn new(timeout: Duration, acceptor: A) -> Self {
        Self { acceptor, timeout }
    }
}

impl<I: Send, S: Send, A> Accept<I, S> for TimeoutAcceptor<A>
where
    A: Accept<I, S>,
{
    type Stream = A::Stream;
    type Service = A::Service;

    #[inline]
    fn accept(
        &self,
        stream: I,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send {
        use std::{
            marker::PhantomData,
            pin::Pin,
            task::{Context, Poll},
        };

        // Heavily optimized `map` future to handle the timeout case with no extra state.

        #[repr(transparent)]
        #[pin_project::pin_project]
        struct Accepting<F, T>(#[pin] tokio::time::Timeout<F>, PhantomData<fn() -> T>);

        impl<F, T> Future for Accepting<F, T>
        where
            F: Future<Output = io::Result<T>>,
        {
            type Output = io::Result<T>;

            #[inline]
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Ready(match self.project().0.poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(res)) => res,
                    Poll::Ready(Err(_)) => Err(io::Error::new(io::ErrorKind::TimedOut, "accept timed out")),
                })
            }
        }

        Accepting(
            tokio::time::timeout(self.timeout, self.acceptor.accept(stream, service)),
            PhantomData,
        )
    }
}
