use super::accept::{Accept, DefaultAcceptor};
use crate::error::io_other;

use arc_swap::ArcSwap;
use rustls::ServerConfig;
use rustls_pemfile::Item;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::future::Future;
use std::io::ErrorKind;
use std::time::Duration;
use std::{fmt, io, net::SocketAddr, path::Path, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    task::spawn_blocking,
};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

/// Tls acceptor using rustls.
#[derive(Clone)]
pub struct RustlsAcceptor<A = DefaultAcceptor> {
    inner: A,
    config: RustlsConfig,
    handshake_timeout: Duration,
}

impl RustlsAcceptor {
    /// Create a new rustls acceptor.
    pub fn new(config: RustlsConfig) -> Self {
        let inner = DefaultAcceptor;

        #[cfg(not(test))]
        let handshake_timeout = Duration::from_secs(10);

        // Don't force tests to wait too long.
        #[cfg(test)]
        let handshake_timeout = Duration::from_secs(1);

        Self {
            inner,
            config,
            handshake_timeout,
        }
    }

    /// Override the default TLS handshake timeout of 10 seconds, except during testing.
    pub fn handshake_timeout(mut self, val: Duration) -> Self {
        self.handshake_timeout = val;
        self
    }
}

impl<A> RustlsAcceptor<A> {
    /// Overwrite inner acceptor.
    pub fn acceptor<Acceptor>(self, acceptor: Acceptor) -> RustlsAcceptor<Acceptor> {
        RustlsAcceptor {
            inner: acceptor,
            config: self.config,
            handshake_timeout: self.handshake_timeout,
        }
    }
}

impl<A, I: Send, S: Send> Accept<I, S> for RustlsAcceptor<A>
where
    A: Accept<I, S>,
{
    type Stream = TlsStream<A::Stream>;
    type Service = A::Service;

    fn accept(
        &self,
        stream: I,
        service: S,
    ) -> impl Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send {
        async move {
            let (stream, service) = self.inner.accept(stream, service).await?;

            let handshake = tokio::time::timeout(
                self.handshake_timeout,
                TlsAcceptor::from(self.config.get_inner()).accept(stream),
            );

            match handshake.await {
                Ok(Ok(stream)) => Ok((stream, service)),
                Ok(Err(e)) => Err(e),
                Err(timeout) => Err(io::Error::new(ErrorKind::TimedOut, timeout)),
            }
        }
    }
}

impl<A> fmt::Debug for RustlsAcceptor<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsAcceptor").finish()
    }
}

/// Rustls configuration.
#[derive(Clone)]
pub struct RustlsConfig {
    inner: Arc<ArcSwap<ServerConfig>>,
}

impl RustlsConfig {
    /// Create config from `Arc<`[`ServerConfig`]`>`.
    ///
    /// NOTE: You need to set ALPN protocols (like `http/1.1` or `h2`) manually.
    pub fn from_config(config: Arc<ServerConfig>) -> Self {
        let inner = Arc::new(ArcSwap::new(config));

        Self { inner }
    }

    /// Create config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    pub async fn from_der(cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<Self> {
        let server_config = spawn_blocking(|| config_from_der(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Create config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    pub async fn from_pem(cert: Vec<u8>, key: Vec<u8>) -> io::Result<Self> {
        let server_config = spawn_blocking(|| config_from_pem(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Create config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    pub async fn from_pem_file(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> io::Result<Self> {
        let server_config = config_from_pem_file(cert, key).await?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Get  inner `Arc<`[`ServerConfig`]`>`.
    pub fn get_inner(&self) -> Arc<ServerConfig> {
        self.inner.load_full()
    }

    /// Reload config from `Arc<`[`ServerConfig`]`>`.
    pub fn reload_from_config(&self, config: Arc<ServerConfig>) {
        self.inner.store(config);
    }

    /// Reload config from DER-encoded data.
    ///
    /// The certificate must be DER-encoded X.509.
    ///
    /// The private key must be DER-encoded ASN.1 in either PKCS#8 or PKCS#1 format.
    pub async fn reload_from_der(&self, cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<()> {
        let server_config = spawn_blocking(|| config_from_der(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(server_config);

        self.inner.store(inner);

        Ok(())
    }

    /// This helper will establish a TLS server based on strong cipher suites
    /// from a PEM-formatted certificate chain and key.
    pub async fn from_pem_chain_file(
        chain: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> io::Result<Self> {
        let server_config = config_from_pem_chain_file(chain, key).await?;
        let inner = Arc::new(ArcSwap::from_pointee(server_config));

        Ok(Self { inner })
    }

    /// Reload config from PEM formatted data.
    ///
    /// Certificate and private key must be in PEM format.
    pub async fn reload_from_pem(&self, cert: Vec<u8>, key: Vec<u8>) -> io::Result<()> {
        let server_config = spawn_blocking(|| config_from_pem(cert, key))
            .await
            .unwrap()?;
        let inner = Arc::new(server_config);

        self.inner.store(inner);

        Ok(())
    }

    /// Reload config from PEM formatted files.
    ///
    /// Contents of certificate file and private key file must be in PEM format.
    pub async fn reload_from_pem_file(
        &self,
        cert: impl AsRef<Path>,
        key: impl AsRef<Path>,
    ) -> io::Result<()> {
        let server_config = config_from_pem_file(cert, key).await?;
        let inner = Arc::new(server_config);

        self.inner.store(inner);

        Ok(())
    }
}

impl fmt::Debug for RustlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsConfig").finish()
    }
}

fn config_from_der(cert: Vec<Vec<u8>>, key: Vec<u8>) -> io::Result<ServerConfig> {
    let cert = cert.into_iter().map(CertificateDer::from).collect();
    let key = PrivateKeyDer::try_from(key).map_err(io_other)?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert, key)
        .map_err(io_other)?;

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(config)
}

fn config_from_pem(cert: Vec<u8>, key: Vec<u8>) -> io::Result<ServerConfig> {
    let cert = rustls_pemfile::certs(&mut cert.as_ref())
        .map(|it| it.map(|it| it.to_vec()))
        .collect::<Result<Vec<_>, _>>()?;
    // Check the entire PEM file for the key in case it is not first section
    let mut key_vec: Vec<Vec<u8>> = rustls_pemfile::read_all(&mut key.as_ref())
        .filter_map(|i| match i.ok()? {
            Item::Sec1Key(key) => Some(key.secret_sec1_der().to_vec()),
            Item::Pkcs1Key(key) => Some(key.secret_pkcs1_der().to_vec()),
            Item::Pkcs8Key(key) => Some(key.secret_pkcs8_der().to_vec()),
            _ => None,
        })
        .collect();

    // Make sure file contains only one key
    if key_vec.len() != 1 {
        return Err(io_other("private key format not supported"));
    }

    config_from_der(cert, key_vec.pop().unwrap())
}

async fn config_from_pem_file(
    cert: impl AsRef<Path>,
    key: impl AsRef<Path>,
) -> io::Result<ServerConfig> {
    let cert = tokio::fs::read(cert.as_ref()).await?;
    let key = tokio::fs::read(key.as_ref()).await?;

    config_from_pem(cert, key)
}

async fn config_from_pem_chain_file(
    cert: impl AsRef<Path>,
    chain: impl AsRef<Path>,
) -> io::Result<ServerConfig> {
    let cert = tokio::fs::read(cert.as_ref()).await?;
    let cert = rustls_pemfile::certs(&mut cert.as_ref())
        .map(|it| it.map(|it| CertificateDer::from(it.to_vec())))
        .collect::<Result<Vec<_>, _>>()?;
    let key = tokio::fs::read(chain.as_ref()).await?;
    let key_cert: PrivateKeyDer = match rustls_pemfile::read_one(&mut key.as_ref())?
        .ok_or_else(|| io_other("could not parse pem file"))?
    {
        Item::Pkcs8Key(key) => Ok(key.into()),
        Item::Sec1Key(key) => Ok(key.into()),
        Item::Pkcs1Key(key) => Ok(key.into()),
        x => Err(io_other(format!(
            "invalid certificate format, received: {x:?}"
        ))),
    }?;

    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert, key_cert)
        .map_err(|_| io_other("invalid certificate"))
}
