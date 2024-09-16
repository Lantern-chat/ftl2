use std::io;
use std::sync::Arc;

use http::header;
use http_body::Frame;
use http_body_util::BodyStream;
use tokio_stream::StreamExt;
use tokio_util::io::{ReaderStream, StreamReader};

pub use async_compression::Level;

use crate::body::{Body, BodyError};
use crate::headers::accept_encoding::{AcceptEncoding, Encoding};
use crate::{Layer, Service};

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

        async move {
            let (mut parts, body) = inner.await?.into_parts();

            let should_compress = !parts.headers.contains_key(header::CONTENT_ENCODING)
                && !parts.headers.contains_key(header::CONTENT_RANGE)
                && self.layer.predicate.should_compress(&parts);

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

            let map = move |r: Result<_, io::Error>| match r {
                Ok(data) => Ok(Frame::data(data)),
                Err(e) => match e.downcast::<<RespBody as http_body::Body>::Error>() {
                    // TODO: Handle internal body errors better?
                    Ok(e) => Err(BodyError::Generic(e.into())),
                    Err(e) => Err(BodyError::Io(e)),
                },
            };

            let trailers = futures::stream::unfold((false, orig_trailers), move |(checked, ot)| async move {
                if checked {
                    return None; // don't bother locking if we've already yielded the trailers
                }

                let trailers = ot.lock().unwrap().take();

                trailers.map(|trailers| (Ok(trailers), (true, ot)))
            });

            use async_compression::tokio::bufread::{BrotliEncoder, DeflateEncoder, GzipEncoder, ZstdEncoder};

            let compressed = match encoding {
                Encoding::Identity => unreachable!(),
                Encoding::Deflate => Body::stream(
                    ReaderStream::new(DeflateEncoder::with_quality(stream, self.layer.level))
                        .map(map)
                        .chain(trailers),
                ),
                Encoding::Gzip => Body::stream(
                    ReaderStream::new(GzipEncoder::with_quality(stream, self.layer.level))
                        .map(map)
                        .chain(trailers),
                ),
                Encoding::Brotli => Body::stream({
                    // The brotli crate used under the hood here has a default compression level of 11,
                    // which is the max for brotli. This causes extremely slow compression times, so we
                    // manually set a default of 4 here.
                    //
                    // This is the same default used by NGINX for on-the-fly brotli compression.
                    let level = match self.layer.level {
                        Level::Default => Level::Precise(4),
                        level => level,
                    };

                    ReaderStream::new(BrotliEncoder::with_quality(stream, level)).map(map).chain(trailers)
                }),
                Encoding::Zstd => Body::stream({
                    // See https://issues.chromium.org/issues/41493659:
                    //  "For memory usage reasons, Chromium limits the window size to 8MB"
                    // See https://datatracker.ietf.org/doc/html/rfc8878#name-window-descriptor
                    //  "For improved interoperability, it's recommended for decoders to support values
                    //  of Window_Size up to 8 MB and for encoders not to generate frames requiring a
                    //  Window_Size larger than 8 MB."
                    // Level 17 in zstd (as of v1.5.6) is the first level with a window size of 8 MB (2^23):
                    // https://github.com/facebook/zstd/blob/v1.5.6/lib/compress/clevels.h#L25-L51
                    // Set the parameter for all levels >= 17. This will either have no effect (but reduce
                    // the risk of future changes in zstd) or limit the window log to 8MB.
                    let needs_window_limit = match self.layer.level {
                        Level::Best => true, // 20
                        Level::Precise(level) => level >= 17,
                        _ => false,
                    };

                    // The parameter is not set for levels below 17 as it will increase the window size
                    // for those levels.
                    let params: &[_] = if needs_window_limit {
                        &[async_compression::zstd::CParameter::window_log(23)]
                    } else {
                        &[]
                    };

                    ReaderStream::new(ZstdEncoder::with_quality_and_params(stream, self.layer.level, params))
                        .map(map)
                        .chain(trailers)
                }),
            };

            parts.headers.remove(header::ACCEPT_RANGES);
            parts.headers.remove(header::CONTENT_LENGTH);

            parts.headers.insert(header::CONTENT_ENCODING, encoding.into_header_value());

            Ok(http::Response::from_parts(parts, compressed))
        }
    }
}
