use std::time::Duration;

use ftl::{
    extract::{real_ip::RealIpPrivacyMask, Extension, MatchedPath},
    layers::{
        compression::CompressionLayer,
        rate_limit::{gcra::Quota, RateLimitLayerBuilder},
    },
    router::Router,
    serve::{
        accept::NoDelayAcceptor,
        tls_openssl::{OpenSSLAcceptor, OpenSSLConfig},
        tls_rustls::{RustlsAcceptor, RustlsConfig},
        Server, TlsConfig,
    },
    service::FtlServiceToHyperMakeService,
    Layer,
};

use ftl::extract::real_ip::{RealIp, RealIpLayer};

use tokio::signal::ctrl_c;

type Key = (RealIpPrivacyMask,); // note there can be multiple keys, but for this example we only use the IP address

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // load tls config from pem files
    let tls_config = RustlsConfig::from_pem_file("cert.pem", "key.pem").await.unwrap();

    // create Router with empty state, could be replaced with any type
    let mut router = Router::with_state(());

    let rate_limit = RateLimitLayerBuilder::<Key>::new()
        .with_default_quota(Quota::simple(Duration::from_secs(5)))
        .with_extension(true)
        .with_global_fallback(true)
        .with_gc_interval(Duration::from_secs(5))
        .default_handle_error();

    // setup routes
    router.get("/{*path}", placeholder).get("/", placeholder).get("/hello", placeholder);

    // create server to bind at localhost:8083, under https
    let mut server = Server::bind("0.0.0.0:8083".parse().unwrap());

    // setup graceful shutdown on ctrl-c
    server.handle().shutdown_on(async { _ = ctrl_c().await });

    // configure the server properties, such as HTTP/2 adaptive window and connect protocol
    server.http2().adaptive_window(true).enable_connect_protocol(); // used for HTTP/2 Websockets

    let service = router.route_layer(rate_limit);

    // serve the router service with the server
    _ = server
        .acceptor(RustlsAcceptor::new(tls_config).acceptor(NoDelayAcceptor))
        .serve(FtlServiceToHyperMakeService::new(
            // Convert the `Incoming` body to FTL Body type and call the service
            (RealIpLayer, CompressionLayer::new()).layer(service),
        ))
        .await;
}

async fn placeholder(
    Extension(p): Extension<MatchedPath>,
    uri: http::Uri,
    Extension(real_ip): Extension<RealIp>,
    _body: ftl::body::Body,
) -> String {
    let long = "Hello".repeat(1000);

    format!("Matched: {}: {} from {}\n\n{long}", p.0, uri.path(), real_ip)
}
