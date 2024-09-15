use std::io;
use std::sync::Arc;

use http::header;
use http_body::Frame;
use http_body_util::{BodyStream, StreamBody};
use tokio_stream::StreamExt;
use tokio_util::io::{ReaderStream, StreamReader};

pub use async_compression::Level;

use crate::body::Body;
use crate::error::io_other;
use crate::headers::accept_encoding::{AcceptEncoding, Encoding};
use crate::{IntoResponse, Layer, Service};

pub mod predicate;

use predicate::{DefaultPredicate, Predicate};

#[derive(Clone, Copy)]
#[must_use]
pub struct CompressionLayer<P: Predicate = DefaultPredicate> {
    accept: AcceptEncoding,
    predicate: P,
    level: Level,
}

impl Default for CompressionLayer<DefaultPredicate> {
    fn default() -> Self {
        Self {
            accept: AcceptEncoding::default(),
            predicate: DefaultPredicate,
            level: Level::Default,
        }
    }
}

#[derive(Clone, Copy)]
#[must_use]
pub struct Compression<S, P: Predicate = DefaultPredicate> {
    inner: S,
    layer: CompressionLayer<P>,
}

impl<S, P: Predicate> Compression<S, P> {
    pub fn layer() -> CompressionLayer {
        CompressionLayer::default()
    }
}

impl CompressionLayer {
    /// Creates a new [`CompressionLayer`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets whether to enable the gzip encoding.
    #[cfg(feature = "compression-gzip")]
    pub fn gzip(mut self, enable: bool) -> Self {
        self.accept.set_gzip(enable);
        self
    }

    /// Sets whether to enable the Deflate encoding.
    #[cfg(feature = "compression-deflate")]
    pub fn deflate(mut self, enable: bool) -> Self {
        self.accept.set_deflate(enable);
        self
    }

    /// Sets whether to enable the Brotli encoding.
    #[cfg(feature = "compression-br")]
    pub fn br(mut self, enable: bool) -> Self {
        self.accept.set_br(enable);
        self
    }

    /// Sets whether to enable the Zstd encoding.
    #[cfg(feature = "compression-zstd")]
    pub fn zstd(mut self, enable: bool) -> Self {
        self.accept.set_zstd(enable);
        self
    }

    /// Sets the compression level.
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Disables the gzip encoding.
    ///
    /// This method is available even if the `gzip` crate feature is disabled.
    pub fn no_gzip(mut self) -> Self {
        self.accept.set_gzip(false);
        self
    }

    /// Disables the Deflate encoding.
    ///
    /// This method is available even if the `deflate` crate feature is disabled.
    pub fn no_deflate(mut self) -> Self {
        self.accept.set_deflate(false);
        self
    }

    /// Disables the Brotli encoding.
    ///
    /// This method is available even if the `br` crate feature is disabled.
    pub fn no_br(mut self) -> Self {
        self.accept.set_br(false);
        self
    }

    /// Disables the Zstd encoding.
    ///
    /// This method is available even if the `zstd` crate feature is disabled.
    pub fn no_zstd(mut self) -> Self {
        self.accept.set_zstd(false);
        self
    }

    /// Replace the current compression predicate.
    ///
    /// See [`Compression::compress_when`] for more details.
    pub fn compress_when<C>(self, predicate: C) -> CompressionLayer<C>
    where
        C: Predicate,
    {
        CompressionLayer {
            accept: self.accept,
            predicate,
            level: self.level,
        }
    }
}

impl<S, P> Layer<S> for CompressionLayer<P>
where
    P: Predicate,
{
    type Service = Compression<S, P>;

    fn layer(&self, inner: S) -> Self::Service {
        Compression {
            inner,
            layer: self.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompressionError<E> {
    #[error(transparent)]
    Inner(E),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("other error")]
    Other,
}

impl<E> IntoResponse for CompressionError<E>
where
    E: IntoResponse,
{
    fn into_response(self) -> crate::Response {
        match self {
            CompressionError::Inner(e) => e.into_response(),
            CompressionError::Io(e) => e.into_response(),
            CompressionError::Other => {
                ("Internal Server Error", http::StatusCode::INTERNAL_SERVER_ERROR).into_response()
            }
        }
    }
}

impl<S, P, ReqBody, RespBody> Service<http::Request<ReqBody>> for Compression<S, P>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<RespBody>>,
    RespBody:
        http_body::Body<Data = bytes::Bytes, Error: std::error::Error + Send + Sync + 'static> + Send + 'static,
    P: Predicate,
{
    type Response = crate::Response;
    type Error = S::Error;

    fn call(
        &self,
        req: http::Request<ReqBody>,
    ) -> impl crate::service::ServiceFuture<Self::Response, Self::Error> {
        let encoding = Encoding::from_headers(req.headers(), self.layer.accept);

        let inner = self.inner.call(req);

        let layer = self.layer.clone();

        async move {
            let (mut parts, body) = inner.await?.into_parts();

            let should_compress = !parts.headers.contains_key(header::CONTENT_ENCODING)
                && !parts.headers.contains_key(header::CONTENT_RANGE)
                && layer.predicate.should_compress(&parts);

            if should_compress {
                parts.headers.append(header::VARY, header::ACCEPT_ENCODING.into());
            }

            if !should_compress || encoding == Encoding::Identity {
                return Ok(http::Response::from_parts(parts, Body::from_any_body(body)));
            }

            use std::sync::Mutex;

            let orig_trailers = Arc::new(Mutex::new(None));
            let ot = orig_trailers.clone();

            let stream = StreamReader::new(BodyStream::new(body).map(move |frame| match frame {
                Err(e) => Err(io::Error::other(e)),
                Ok(frame) => Ok(match frame.into_data() {
                    Ok(data) => data,
                    Err(trailers) => {
                        *ot.lock().unwrap() = Some(trailers);
                        bytes::Bytes::new()
                    }
                }),
            }));

            use async_compression::tokio::bufread::{BrotliEncoder, DeflateEncoder, GzipEncoder, ZstdEncoder};

            let map = move |r| match r {
                Ok(data) => Ok(Frame::data(data)),
                Err(e) => Err(crate::body::BodyError::Io(e)),
                // Err(e) => match e.downcast::<<B as http_body::Body>::Error>() {
                //     Ok(e) => Err(CompressionError::Inner(e)),
                //     Err(e) => Err(CompressionError::Io(e)),
                // },
            };

            let trailers = futures::stream::unfold(orig_trailers, |ot| async move {
                let trailers = ot.lock().unwrap().take();

                trailers.map(|trailers| (Ok(trailers), ot))
            });

            let compressed = match encoding {
                Encoding::Gzip => Body::stream(
                    ReaderStream::new(GzipEncoder::with_quality(stream, layer.level)).map(map).chain(trailers),
                ),
                Encoding::Deflate => Body::stream(
                    ReaderStream::new(DeflateEncoder::with_quality(stream, layer.level)).map(map).chain(trailers),
                ),
                Encoding::Brotli => Body::stream(
                    ReaderStream::new(BrotliEncoder::with_quality(stream, layer.level)).map(map).chain(trailers),
                ),
                Encoding::Zstd => Body::stream(
                    ReaderStream::new(ZstdEncoder::with_quality(stream, layer.level)).map(map).chain(trailers),
                ),
                Encoding::Identity => unreachable!(),
            };

            parts.headers.remove(header::ACCEPT_RANGES);
            parts.headers.remove(header::CONTENT_LENGTH);

            parts.headers.insert(header::CONTENT_ENCODING, encoding.into_header_value());

            Ok(http::Response::from_parts(parts, compressed))
        }
    }
}
