use futures::TryFutureExt;

use crate::{service::ServiceFuture, Layer, Response, Service};

/// The encoding to use for serialization of deferred values.
///
/// Defaults to JSON if both JSON and CBOR features are enabled,
/// or CBOR if only the CBOR feature is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// JSON encoding
    #[cfg(feature = "json")]
    Json,

    /// CBOR encoding
    #[cfg(feature = "cbor")]
    Cbor,
}

impl Default for Encoding {
    #[allow(unreachable_code)]
    fn default() -> Self {
        #[cfg(feature = "json")]
        return Encoding::Json;

        #[cfg(feature = "cbor")]
        return Encoding::Cbor;
    }
}

/// A layer that defers encoding of [`Deferred`] values to
/// use a specific encoding given in the request.
///
/// [`Deferred`]: crate::body::deferred::Deferred
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[must_use]
pub struct DeferredEncoding {
    default_encoding: Encoding,
    fields: &'static [&'static str],
}

/// The service created by the [`DeferredEncoding`] layer.
#[derive(Debug, Clone)]
pub struct DeferredEncodingService<S> {
    service: S,
    default_encoding: Encoding,
    fields: &'static [&'static str],
}

impl Default for DeferredEncoding {
    fn default() -> Self {
        DeferredEncoding {
            default_encoding: Encoding::default(),
            fields: &["encoding"],
        }
    }
}

impl DeferredEncoding {
    /// Overrides the default encoding to use for serialization.
    pub fn with_default_encoding(self, default_encoding: Encoding) -> Self {
        DeferredEncoding {
            default_encoding,
            ..self
        }
    }

    /// Sets the query fields to look for the encoding parameter in. The default is `&["encoding"]`, meaning
    /// the query parameter `encoding` will be used to determine the encoding, e.g. `?encoding=json`.
    ///
    /// This should not contain characters that would normally be urlencoded, as the query is not decoded
    /// at the time of parsing (for this layer).
    pub fn with_query_fields(self, fields: &'static [&'static str]) -> Self {
        DeferredEncoding { fields, ..self }
    }
}

impl<S> Layer<S> for DeferredEncoding {
    type Service = DeferredEncodingService<S>;

    fn layer(&self, service: S) -> Self::Service {
        DeferredEncodingService {
            service,
            default_encoding: self.default_encoding,
            fields: self.fields,
        }
    }
}

impl<S, B> Service<http::Request<B>> for DeferredEncodingService<S>
where
    S: Service<http::Request<B>, Response = Response>,
{
    type Error = S::Error;
    type Response = Response;

    #[inline]
    fn call(&self, req: http::Request<B>) -> impl ServiceFuture<Self::Response, Self::Error> {
        // `PathAndQuery` is cheap to clone, so do that to avoid parsing until later,
        // since not all requests will need it.
        let path = req.uri().path_and_query().cloned();

        self.service.call(req).map_ok(move |res: Response| {
            use crate::body::{Body, BodyInner};

            let (mut parts, body) = res.into_parts();

            match body.0 {
                BodyInner::Deferred(deferred) => {
                    let mut encoding = self.default_encoding;

                    if let Some(query) = path.as_ref().and_then(|path| path.query()) {
                        // because neither the key or value we care about are urlencoded, we can just do simple splits
                        for pair in query.split('&') {
                            if let Some((key, value)) = pair.split_once('=') {
                                if !self.fields.contains(&key) {
                                    continue;
                                }

                                encoding = match value {
                                    #[cfg(feature = "json")]
                                    "json" => Encoding::Json,

                                    #[cfg(feature = "cbor")]
                                    "cbor" => Encoding::Cbor,

                                    _ => continue,
                                };

                                break;
                            }
                        }
                    }

                    let (new_parts, body) = deferred.0.into_response(encoding).into_parts();

                    if !new_parts.status.is_success() {
                        parts = new_parts;
                    } else {
                        parts.headers.extend(new_parts.headers);
                        parts.extensions.extend(new_parts.extensions);
                    }

                    Response::from_parts(parts, body)
                }
                inner => Response::from_parts(parts, Body(inner)),
            }
        })
    }
}
