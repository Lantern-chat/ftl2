use std::time::Duration;

use ftl::{
    body::Json,
    extract::{real_ip::RealIpPrivacyMask, Extension, MatchedPath},
    layers::{
        compression::CompressionLayer,
        rate_limit::{gcra::Quota, RateLimitLayerBuilder},
    },
    rewrite::RewriteService,
    router::Router,
    serve::{
        accept::NoDelayAcceptor,
        // tls_openssl::{OpenSSLAcceptor, OpenSSLConfig},
        tls_rustls::{RustlsAcceptor, RustlsConfig},
        Server,
        TlsConfig,
    },
    service::FtlServiceToHyperMakeService,
    IntoResponse, Layer,
};

use ftl::extract::real_ip::{RealIp, RealIpLayer};

use futures::StreamExt;
use tokio::signal::ctrl_c;

type RateLimitKey = (RealIpPrivacyMask,); // note there can be multiple keys, but for this example we only use the IP address

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // load tls config from pem files
    let tls_config = RustlsConfig::from_pem_file("ecdsa_certificate.pem", "ecdsa_private_key.pem").await.unwrap();

    // create Router with empty state, could be replaced with any type
    let mut router = Router::with_state(());

    // setup rate limit layer
    let rate_limit = RateLimitLayerBuilder::<RateLimitKey>::new()
        .with_default_quota(Quota::simple(Duration::from_secs(5)))
        .with_extension(true)
        .with_global_fallback(true)
        .with_gc_interval(Duration::from_secs(5))
        .default_handle_error();

    // setup routes
    router
        .get("/{*path}", placeholder)
        .get("/", placeholder)
        .get("/hello", placeholder)
        .get("/test", test);

    // create server to bind at localhost:8083, under https
    let mut server = Server::bind(["0.0.0.0:8083".parse().unwrap()]);

    let handle = server.handle();

    // setup graceful shutdown on ctrl-c
    server.handle().shutdown_on(async { _ = ctrl_c().await });
    server.handle().set_shutdown_timeout(Some(Duration::from_secs(5)));

    // configure the server properties, such as HTTP/2 adaptive window and connect protocol
    server.http2().adaptive_window(true).enable_connect_protocol(); // used for HTTP/2 Websockets

    // create a redirect server to bind at localhost:8080, under http, whilst sharing the same underlying Handle and config
    let redirect_server = server.rebind(["0.0.0.0:8080".parse().unwrap()]);

    // serve the router service with the server
    tokio::spawn(
        // set acceptor to use the tls config, and set that acceptor to use NoDelay
        server.acceptor(RustlsAcceptor::new(tls_config).acceptor(NoDelayAcceptor)).serve(
            FtlServiceToHyperMakeService::new(
                // Convert the `Incoming` body to FTL Body type and call the service
                (RealIpLayer, CompressionLayer::new()).layer(router.route_layer(rate_limit)),
            ),
        ),
    );

    tokio::spawn(
        // setup a redirect server to redirect all http traffic to https
        redirect_server.serve(FtlServiceToHyperMakeService::new(RewriteService::permanent(|parts| {
            let host = parts.extensions.get::<http::uri::Authority>().unwrap();

            format!("https://{}:8083{}", host.host(), parts.uri.path())
        }))),
    );

    // wait for the servers to finish
    handle.wait().await;
}

async fn placeholder(
    Extension(p): Extension<MatchedPath>,
    uri: http::Uri,
    Extension(real_ip): Extension<RealIp>,
    _body: ftl::body::Body,
) -> String {
    let long = "Hello".repeat(100000);

    format!("Matched: {}: {} from {}\n\n{long}", p.0, uri.path(), real_ip)
}

use serde::Serialize;

async fn test() -> impl IntoResponse {
    #[derive(Serialize)]
    struct Test {
        a: i32,
        b: String,
    }

    let mut i = 0;

    Json::stream_simple_array(
        futures::stream::repeat_with(move || Test {
            a: {
                i += 1;
                i
            },
            b: "test".to_string(),
        })
        .take(100),
    )
}
