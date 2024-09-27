use headers::HeaderMapExt;
use http_body::{Body, Frame, SizeHint};

use bytes::{BufMut, Bytes, BytesMut};
use std::task::{Context, Poll};
use std::time::Instant;
use std::{io, pin::Pin};
use tokio::io::{AsyncRead, ReadBuf};

use crate::headers::server_timing::{ServerTiming, ServerTimings};

enum State<R> {
    Reading(R),
    Trailers,
    Finished,
}

impl<R> State<R> {
    // Similar to Option::as_pin_mut
    fn as_pin_mut(self: Pin<&mut Self>) -> State<Pin<&mut R>> {
        unsafe {
            match Pin::get_unchecked_mut(self) {
                State::Reading(ref mut r) => State::Reading(Pin::new_unchecked(r)),
                State::Trailers => State::Trailers,
                State::Finished => State::Finished,
            }
        }
    }
}

/// HTTP `Body` created from an `AsyncRead`, reading in chunks of `Bytes`,
/// up to a specified length or until the end.
///
/// The body will return trailers with the server-timing header
/// containing the duration of the request.
///
/// Furthermore, the capacity of the buffer can be specified to control the
/// amount of memory allocated for each read operation.
#[pin_project::pin_project]
pub struct AsyncReadBody<R> {
    #[pin]
    reader: State<R>,

    buf: BytesMut,
    capacity: usize,
    len: u64,

    start: Instant,
}

impl<R: AsyncRead> AsyncReadBody<R> {
    /// Set len to `u64::MAX` to read until EOF.
    pub fn new(reader: R, capacity: usize, start: Instant, len: u64) -> Self {
        AsyncReadBody {
            reader: State::Reading(reader),
            buf: BytesMut::new(),
            capacity,
            len,
            start,
        }
    }
}

impl<R: AsyncRead> Body for AsyncReadBody<R> {
    type Data = Bytes;
    type Error = io::Error;

    #[inline]
    fn is_end_stream(&self) -> bool {
        matches!(self.reader, State::Finished)
    }

    fn size_hint(&self) -> SizeHint {
        match self.reader {
            State::Reading(_) => {
                if self.len != u64::MAX {
                    SizeHint::default()
                } else {
                    SizeHint::with_exact(self.len)
                }
            }
            _ => SizeHint::with_exact(0),
        }
    }

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_mut().project();

        let reader = match this.reader.as_pin_mut() {
            State::Reading(r) => r,
            State::Finished => return Poll::Ready(None),
            State::Trailers => {
                let timings = ServerTimings::new().with(ServerTiming::new("end").elapsed_from(*this.start));

                self.project().reader.set(State::Finished);

                let mut trailers = http::HeaderMap::new();
                trailers.typed_insert(timings);

                if trailers.is_empty() {
                    return Poll::Ready(None); // skip if timings didn't encode properly
                }

                return Poll::Ready(Some(Ok(Frame::trailers(trailers))));
            }
        };

        if this.buf.capacity() == 0 {
            this.buf.reserve((*this.len).min(*this.capacity as u64) as usize);
        }

        let buf = this.buf;

        let n = 'poll_read_buf: {
            if !buf.has_remaining_mut() {
                break 'poll_read_buf 0;
            }

            //let mut chunk_buf = unsafe { &mut *(buf.chunk_mut() as *mut _ as *mut [MaybeUninit<u8>]) };

            // same as above for BytesMut, but without a chance of reserving more space
            let mut chunk_buf = buf.spare_capacity_mut();

            // if applicable, limit the buffer to the remaining length
            if *this.len < (chunk_buf.len() as u64) {
                chunk_buf = &mut chunk_buf[..*this.len as usize];
            }

            let mut buf = ReadBuf::uninit(chunk_buf);
            let ptr = buf.filled().as_ptr();

            // TODO: Is poll_read guaranteed to fill the buffer?
            match reader.poll_read(cx, &mut buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(err)) => {
                    // set the reader to finished to prevent further reads on error
                    self.project().reader.set(State::Finished);
                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Ready(Ok(_)) => {
                    let filled = buf.filled();

                    // Ensure the pointer does not change from under us
                    assert_eq!(ptr, filled.as_ptr());

                    filled.len()
                }
            }
        };

        unsafe { buf.advance_mut(n) };
        *this.len -= n as u64;

        let frame = Frame::data(buf.split().freeze());

        // if length is empty we've read as much as requested,
        // or if the bytes read is 0 we hit EOF
        if *this.len == 0 || n == 0 {
            self.project().reader.set(State::Trailers);
        }

        Poll::Ready(Some(Ok(frame)))
    }
}
