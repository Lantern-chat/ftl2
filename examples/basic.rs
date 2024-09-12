use ftl::{
    extract::{Extension, MatchedPath},
    router::Router,
    serve::tls_rustls::{RustlsAcceptor, RustlsConfig},
    serve::{accept::NoDelayAcceptor, Server},
    service::FtlServiceToHyperMakeService,
};

#[tokio::main]
async fn main() {
    let tls_config = RustlsConfig::from_pem_file("cert.pem", "key.pem")
        .await
        .unwrap();

    let mut router = Router::with_state(());

    router.get("/{*path}", placeholder).get("/", placeholder);

    let mut server = Server::bind("127.0.0.1:8083".parse().unwrap());

    server.http_builder().http1().http2().adaptive_window(true);

    _ = server
        .acceptor(RustlsAcceptor::new(tls_config).acceptor(NoDelayAcceptor))
        .serve(FtlServiceToHyperMakeService::new(router))
        .await;
}

async fn placeholder(Extension(p): Extension<MatchedPath>, uri: http::Uri) -> String {
    format!("Matched: {}: {}", p.0, uri.path())
}
