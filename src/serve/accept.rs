use std::{future::Future, io};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};

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
