use super::accept::{Accept, DefaultAcceptor};
use crate::error::io_other;

use arc_swap::ArcSwap;
use std::future::{poll_fn, Future};
use std::io::ErrorKind;
use std::pin::Pin;
use std::time::Duration;
use std::{fmt, io, net::SocketAddr, path::Path, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};

use openssl::{
    pkey::PKey,
    ssl::{
        self, AlpnError, Error as OpenSSLError, Ssl, SslAcceptor, SslAcceptorBuilder, SslFiletype, SslMethod,
        SslRef,
    },
    x509::X509,
};
use tokio_openssl::SslStream;

#[derive(Clone)]
#[must_use]
pub struct OpenSSLAcceptor<A = DefaultAcceptor> {
    inner: A,
    config: OpenSSLConfig,
    handshake_timeout: Duration,
}

impl OpenSSLAcceptor {
    /// Create a new OpenSSL acceptor based on the provided [`OpenSSLConfig`]. This is
    /// generally used with manual calls to [`Server::bind`]. You may want [`bind_openssl`]
    /// instead.
    pub fn new(config: OpenSSLConfig) -> Self {
        #[cfg(not(test))]
        let handshake_timeout = Duration::from_secs(10);

        // Don't force tests to wait too long.
        #[cfg(test)]
        let handshake_timeout = Duration::from_secs(1);

        Self {
            inner: DefaultAcceptor,
            config,
            handshake_timeout,
        }
    }

    /// Override the default TLS handshake timeout of 10 seconds.
    pub fn handshake_timeout(mut self, val: Duration) -> Self {
        self.handshake_timeout = val;
        self
    }
}

impl<A> OpenSSLAcceptor<A> {
    /// Overwrite inner acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> OpenSSLAcceptor<Acceptor> {
        OpenSSLAcceptor {
            inner: acceptor,
            config: self.config,
            handshake_timeout: self.handshake_timeout,
        }
    }
}

impl<A, I: Send, S: Send> Accept<I, S> for OpenSSLAcceptor<A>
where
    A: Accept<I, S>,
    A::Stream: AsyncRead + AsyncWrite + Unpin,
{
    type Stream = SslStream<A::Stream>;
    type Service = A::Service;

    fn accept(
        &self,
        stream: I,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send {
        async move {
            let (stream, service) = self.inner.accept(stream, service).await?;

            let handshake = tokio::time::timeout(self.handshake_timeout, async {
                let acceptor = self.config.get_inner();
                let ssl = Ssl::new(acceptor.context()).unwrap();

                let mut tls_stream = SslStream::new(ssl, stream).map_err(io_other)?;

                poll_fn(|cx| Pin::new(&mut tls_stream).poll_accept(cx)).await.map_err(io_other)?;

                Ok(tls_stream)
            });

            match handshake.await {
                Ok(Ok(stream)) => Ok((stream, service)),
                Ok(Err(e)) => Err(e),
                Err(timeout) => Err(io::Error::new(ErrorKind::TimedOut, timeout)),
            }
        }
    }
}

impl<A> fmt::Debug for OpenSSLAcceptor<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSSLAcceptor").finish()
    }
}

/// OpenSSL configuration.
#[derive(Clone)]
#[must_use]
pub struct OpenSSLConfig {
    acceptor: Arc<ArcSwap<SslAcceptor>>,
}

impl OpenSSLConfig {
    /// Create config from `Arc<`[`SslAcceptor`]`>`.
    pub fn from_acceptor(acceptor: Arc<SslAcceptor>) -> Self {
        let acceptor = Arc::new(ArcSwap::new(acceptor));

        OpenSSLConfig { acceptor }
    }

    /// Get inner `Arc<`[`SslAcceptor`]`>`.
    #[must_use]
    pub fn get_inner(&self) -> Arc<SslAcceptor> {
        self.acceptor.load_full()
    }

    /// Reload acceptor from `Arc<`[`SslAcceptor`]`>`.
    pub fn reload_from_acceptor(&self, acceptor: Arc<SslAcceptor>) {
        self.acceptor.store(acceptor);
    }
}

impl super::TlsConfig for OpenSSLConfig {
    type Error = OpenSSLError;
    type DerCert = Vec<u8>;
    type DerCertChain = Vec<u8>;

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a DER-encoded certificate and key.
    async fn from_der(cert: Self::DerCertChain, key: Vec<u8>) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_der(cert.as_ref(), key.as_ref())?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate and key.
    async fn from_pem(cert: String, key: String) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_pem(cert.as_bytes(), key.as_bytes())?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate and key.
    async fn from_pem_file(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_pem_file(cert, key)?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate chain and key.
    async fn from_pem_chain_file(chain: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<Self, OpenSSLError> {
        let acceptor = Arc::new(ArcSwap::from_pointee(config_from_pem_chain_file(chain, key)?));

        Ok(OpenSSLConfig { acceptor })
    }

    /// Reload acceptor from a DER-encoded certificate and key.
    async fn reload_from_der(&self, cert: Self::DerCertChain, key: Vec<u8>) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_der(cert.as_ref(), key.as_ref())?);
        self.acceptor.store(acceptor);

        Ok(())
    }

    /// Reload acceptor from a PEM-formatted certificate and key.
    async fn reload_from_pem(&self, cert: String, key: String) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_pem(cert.as_bytes(), key.as_bytes())?);
        self.acceptor.store(acceptor);

        Ok(())
    }

    /// Reload acceptor from a PEM-formatted certificate and key.
    async fn reload_from_pem_file(
        &self,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_pem_file(cert, key)?);
        self.acceptor.store(acceptor);

        Ok(())
    }

    /// Reload acceptor from a PEM-formatted certificate chain and key.
    async fn reload_from_pem_chain_file(
        &self,
        chain: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> Result<(), OpenSSLError> {
        let acceptor = Arc::new(config_from_pem_chain_file(chain, key)?);
        self.acceptor.store(acceptor);

        Ok(())
    }
}

impl TryFrom<SslAcceptorBuilder> for OpenSSLConfig {
    type Error = OpenSSLError;

    /// Build the [`OpenSSLConfig`] from an [`SslAcceptorBuilder`]. This allows precise
    /// control over the settings that will be used by OpenSSL in this server.
    ///
    /// # Example
    /// ```
    /// use ftl::serve::tls_openssl::OpenSSLConfig;
    /// use openssl::ssl::{SslAcceptor, SslMethod};
    /// use std::convert::TryFrom;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())
    ///         .unwrap();
    ///     // Set configurations like set_certificate_chain_file or
    ///     // set_private_key_file.
    ///     // let tls_builder.set_ ... ;

    ///     let _config = OpenSSLConfig::try_from(tls_builder);
    /// }
    /// ```
    fn try_from(mut tls_builder: SslAcceptorBuilder) -> Result<Self, Self::Error> {
        // Any other checks?
        tls_builder.check_private_key()?;
        tls_builder.set_alpn_select_callback(alpn_select);

        let acceptor = Arc::new(ArcSwap::from_pointee(tls_builder.build()));

        Ok(OpenSSLConfig { acceptor })
    }
}

impl fmt::Debug for OpenSSLConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSSLConfig").finish()
    }
}

fn alpn_select<'a>(_tls: &mut SslRef, client: &'a [u8]) -> Result<&'a [u8], AlpnError> {
    ssl::select_next_proto(b"\x02h2\x08http/1.1", client).ok_or(AlpnError::NOACK)
}

fn config_from_der(cert: &[u8], key: &[u8]) -> Result<SslAcceptor, OpenSSLError> {
    let cert = X509::from_der(cert)?;
    let key = PKey::private_key_from_der(key)?;

    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate(&cert)?;
    tls_builder.set_private_key(&key)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

fn config_from_pem(cert: &[u8], key: &[u8]) -> Result<SslAcceptor, OpenSSLError> {
    let cert = X509::from_pem(cert)?;
    let key = PKey::private_key_from_pem(key)?;

    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate(&cert)?;
    tls_builder.set_private_key(&key)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

fn config_from_pem_file(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<SslAcceptor, OpenSSLError> {
    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate_file(cert, SslFiletype::PEM)?;
    tls_builder.set_private_key_file(key, SslFiletype::PEM)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}

fn config_from_pem_chain_file(
    chain: impl AsRef<Path>,
    key: impl AsRef<Path>,
) -> Result<SslAcceptor, OpenSSLError> {
    let mut tls_builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls())?;
    tls_builder.set_certificate_chain_file(chain)?;
    tls_builder.set_private_key_file(key, SslFiletype::PEM)?;
    tls_builder.check_private_key()?;
    tls_builder.set_alpn_select_callback(alpn_select);

    let acceptor = tls_builder.build();
    Ok(acceptor)
}
