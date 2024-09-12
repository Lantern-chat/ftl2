use ftl::{
    extract::{Extension, MatchedPath},
    router::Router,
    serve::tls_rustls::{RustlsAcceptor, RustlsConfig},
    serve::{accept::NoDelayAcceptor, Server},
    service::FtlServiceToHyperMakeService,
};

use tokio::signal::ctrl_c;

#[tokio::main]
async fn main() {
    // load tls config from pem files
    let tls_config = RustlsConfig::from_pem_file("cert.pem", "key.pem")
        .await
        .unwrap();

    // create Router with empty state, could be replaced with any type
    let mut router = Router::with_state(());

    // setup routes
    router.get("/{*path}", placeholder).get("/", placeholder);

    // create server to bind at localhost:8083, under https
    let mut server = Server::bind("127.0.0.1:8083".parse().unwrap());

    // setup graceful shutdown on ctrl-c
    server.handle().shutdown_on(async { _ = ctrl_c().await });

    // configure the server properties, such as HTTP/2 adaptive window and connect protocol
    server
        .http2()
        .adaptive_window(true)
        .enable_connect_protocol(); // used for HTTP/2 Websockets

    // serve the router service with the server
    _ = server
        .acceptor(RustlsAcceptor::new(tls_config).acceptor(NoDelayAcceptor))
        .serve(FtlServiceToHyperMakeService::new(router))
        .await;
}

async fn placeholder(Extension(p): Extension<MatchedPath>, uri: http::Uri) -> String {
    format!("Matched: {}: {}", p.0, uri.path())
}
