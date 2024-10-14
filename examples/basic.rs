use std::{num::NonZeroU64, time::Duration};

use ftl::{
    body::{deferred::Deferred, Cbor, Json},
    extract::{
        one_of::{OneOf, OneOfAny},
        real_ip::RealIpPrivacyMask,
        Extension, MatchedPath,
    },
    layers::{
        catch_panic::CatchPanic,
        cloneable::Cloneable,
        compression::CompressionLayer,
        convert_body::ConvertBody,
        deferred::DeferredEncoding,
        normalize::Normalize,
        rate_limit::{gcra::Quota, RateLimitLayerBuilder},
        resp_timing::RespTimingLayer,
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
    IntoResponse, Layer, Response,
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
    let mut router = Router::<_, Response>::with_state(());

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
        .get("/test", test)
        .post("/body", with_body)
        .post("/any_body", with_any_body);

    // create server to bind at localhost:8083, under https
    let mut server = Server::bind(["0.0.0.0:8083".parse().unwrap()]);

    let handle = server.handle();

    // setup graceful shutdown on ctrl-c
    server.handle().shutdown_on(async { _ = ctrl_c().await });
    server.handle().set_shutdown_timeout(Duration::from_secs(1));

    // configure the server properties, such as HTTP/2 adaptive window and connect protocol
    server
        .http1()
        .writev(true)
        .pipeline_flush(true)
        .http2()
        .max_concurrent_streams(Some(400))
        .adaptive_window(true)
        .enable_connect_protocol(); // used for HTTP/2 Websockets

    // create a redirect server to bind at localhost:8080, under http, whilst sharing the same underlying Handle and config
    let redirect_server = server.rebind(["0.0.0.0:8080".parse().unwrap()]);

    // spawn the HTTPS server
    tokio::spawn({
        use ftl::serve::accept::{limited::LimitedTcpAcceptor, PeekingAcceptor, TimeoutAcceptor};

        #[rustfmt::skip]
        let acceptor = TimeoutAcceptor::new(
            // 10 second timeout for the entire connection accept process
            Duration::from_secs(10),
            // Accept TLS connections with rustls
            RustlsAcceptor::new(tls_config).acceptor(
                // limit the number of connections per IP to 50
                LimitedTcpAcceptor::new(
                    // TCP_NODELAY, and peek at the first byte of the stream
                    PeekingAcceptor(NoDelayAcceptor),
                    50,
                )
            ),
        );

        // set acceptor to use the tls config, and set that acceptor to use NoDelay
        server.acceptor(acceptor).serve(
            (
                RespTimingLayer::default(),  // logs the time taken to process each request
                CatchPanic::default(),       // spawns each request in a separate task and catches panics
                Cloneable::default(),        // makes the service layered below it cloneable
                RealIpLayer::default(),      // extracts the real ip from the request
                CompressionLayer::new(),     // compresses responses
                Normalize::default(),        // normalizes the response structure
                ConvertBody::default(),      // converts the body to the correct type
                DeferredEncoding::default(), // encodes deferred responses
            )
                .layer(router.route_layer(rate_limit)), // routing layer with per-path rate limiting
        )
    });

    tokio::spawn(
        // setup a redirect server to redirect all http traffic to https
        redirect_server.serve(RewriteService::permanent(|parts| {
            let host = parts.extensions.get::<http::uri::Authority>().unwrap();

            format!("https://{}:8083{}", host.host(), parts.uri.path())
        })),
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

#[derive(serde::Serialize, serde::Deserialize)]
struct Test {
    a: i32,
    b: String,
}

async fn test() -> impl IntoResponse {
    let mut i = 0;

    Deferred::simple_stream(
        futures::stream::repeat_with(move || Test {
            a: {
                i += 1;
                i
            },
            b: if i % 2 == 0 { "even" } else { "odd" }.into(),
        })
        .take(100),
    )
}

async fn with_body(OneOf(value): OneOf<Test, (Json, Cbor)>) {
    println!("{}, {}", value.a, value.b);
}

async fn with_any_body(OneOf(value): OneOfAny<Test>) {
    println!("{}, {}", value.a, value.b);
}
