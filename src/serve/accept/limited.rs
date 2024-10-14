use std::{
    future::Future,
    io,
    net::IpAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

use super::Accept;

type ConnTable = scc::HashIndex<IpAddr, Arc<ConnTracking>, foldhash::fast::RandomState>;

#[derive(Clone)]
pub struct LimitedTcpAcceptor<A> {
    acceptor: A,
    limit: usize,
    conns: Arc<ConnTable>,
}

impl<A> LimitedTcpAcceptor<A> {
    pub fn new(acceptor: A, limit: usize) -> Self {
        Self {
            acceptor,
            limit,
            conns: Arc::new(ConnTable::default()),
        }
    }
}

struct ConnTracking {
    ip: IpAddr,
    count: AtomicUsize,
}

pub struct TrackedTcpStream<I> {
    inner: I,
    conn: Arc<ConnTracking>,
    conns: Arc<ConnTable>,
}

impl<S, A: Accept<TcpStream, S>> Accept<TcpStream, S> for LimitedTcpAcceptor<A>
where
    S: Send,
{
    type Stream = TrackedTcpStream<A::Stream>;
    type Service = A::Service;

    #[inline]
    fn accept(
        &self,
        stream: TcpStream,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send {
        async move {
            let ip = stream.peer_addr()?.ip();

            let (stream, service) = self.acceptor.accept(stream, service).await?;

            let mut failed = false;

            // I know this is convoluted, but it has a happy fast path for when one connection is already established,
            // and the slow path isn't _that_ bad.
            let conn = 'outer: loop {
                let conn = 'inner: {
                    // fast path without locking
                    if !failed {
                        if let Some(conn) = self.conns.peek_with(&ip, |_, conn| conn.clone()) {
                            break 'inner conn;
                        }
                    }

                    match self.conns.entry_async(ip).await {
                        scc::hash_index::Entry::Occupied(occ) => occ.get().clone(),
                        scc::hash_index::Entry::Vacant(vac) => {
                            break 'outer vac
                                .insert_entry(Arc::new(ConnTracking {
                                    ip,
                                    count: AtomicUsize::new(1),
                                }))
                                .get()
                                .clone();
                        }
                    }
                };

                match conn.count.fetch_add(1, Ordering::AcqRel) {
                    // spuriously acquired dead connection, retry with full lock
                    0 => failed = true,

                    // too many connections, drop the count and return an error
                    count if count >= self.limit => {
                        conn.count.fetch_sub(1, Ordering::Relaxed);

                        return Err(io::Error::new(io::ErrorKind::OutOfMemory, "connection limit reached"));
                    }
                    _ => break conn,
                }
            };

            let stream = TrackedTcpStream {
                inner: stream,
                conn,
                conns: self.conns.clone(),
            };

            Ok((stream, service))
        }
    }
}

use std::{
    pin::Pin,
    task::{Context, Poll},
};

impl<I> TrackedTcpStream<I> {
    #[inline(always)]
    fn inner(self: Pin<&mut Self>) -> Pin<&mut I> {
        unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().inner) }
    }
}

impl<I: AsyncRead> AsyncRead for TrackedTcpStream<I> {
    #[inline]
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_read(cx, buf)
    }
}

impl<I: AsyncWrite> AsyncWrite for TrackedTcpStream<I> {
    #[inline]
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.inner().poll_write(cx, buf)
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_flush(cx)
    }

    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner().poll_shutdown(cx)
    }

    #[inline]
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        self.inner().poll_write_vectored(cx, bufs)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

impl<I> Drop for TrackedTcpStream<I> {
    fn drop(&mut self) {
        if self.conn.count.fetch_sub(1, Ordering::AcqRel) > 1 {
            return;
        }

        tokio::task::block_in_place(|| {
            if let Some(occ) = self.conns.get(&self.conn.ip) {
                if occ.get().count.load(Ordering::Acquire) == 0 {
                    occ.remove_entry();
                }
            }
        });
    }
}
