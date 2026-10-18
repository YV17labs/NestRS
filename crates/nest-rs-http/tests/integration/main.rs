//! Integration tests mirroring `src/` — one binary, one module per concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod access_log;
mod allow;
mod body_limit;
mod compression;
mod controller;
mod cors;
mod edge;
mod exclusive_paths;
mod fallback;
mod form;
mod global_prefix;
mod guard_stage;
mod headers;
mod input;
mod json;
mod multipart;
mod opaque;
mod path;
mod pipe;
mod problem;
mod query;
mod response;
mod response_body;
mod route_shaper;
mod routes;
mod security_headers;
mod server;
mod sse;
mod tls;
mod trace_context;
mod transport;
mod versioning;
mod websocket;

use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
use nest_rs_core::{App, Handler, Module, Posture, Transport};
use nest_rs_http::{HttpConfig, HttpTransport};
use nest_rs_testing::TestClient;

/// The suite's own self-mounts.
struct Mounts;

/// A self-mount the transport gates at its edge, as a gateway's upgrade is.
pub(crate) static GATED_MOUNT: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: std::any::TypeId::of::<Mounts>(),
    host_name: "Mounts",
    method: "",
    address: "/",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

/// A self-mount no guard gates, as the OpenAPI document is.
pub(crate) static PUBLIC_MOUNT: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: std::any::TypeId::of::<Mounts>(),
    host_name: "Mounts",
    method: "",
    address: "/",
    posture: Posture::Public,
    meta: MetaLevels::default,
});

/// Boot `M` through a real `HttpTransport` and hand back a client over the
/// composed endpoint; `nest-rs-testing`'s `TestApp` would be a dependency cycle.
pub(crate) async fn boot<M>() -> TestClient
where
    M: Module + 'static,
{
    boot_on::<M>(HttpConfig::default()).await
}

/// Bind `transport` as the kernel's boot does, and the loopback port it took.
pub(crate) async fn listen(transport: &mut HttpTransport) -> u16 {
    let listening = transport.listen().await.expect("the transport listens");
    listening
        .addresses
        .first()
        .expect("the transport listens on one address")
        .port()
}

/// The transport `config` describes, before `configure`.
pub(crate) fn built(config: &HttpConfig) -> HttpTransport {
    HttpTransport::from_config(config).expect("the config is in range")
}

/// The same boot, on the transport `config` describes.
pub(crate) async fn boot_on<M>(config: HttpConfig) -> TestClient
where
    M: Module + 'static,
{
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("module boots");
    let mut transport = built(&config);
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    client(
        transport
            .take_endpoint()
            .expect("configure populates the endpoint"),
    )
}

/// An endpoint answering every request with `body`.
pub(crate) fn answering(body: &'static str) -> nest_rs_http::BoxEndpoint {
    nest_rs_http::BoxEndpoint::new(nest_rs_http::endpoint_fn(move |_| async move {
        Ok(nest_rs_http::Response::builder().body(body))
    }))
}

/// The client the suite drives a composed endpoint through.
pub(crate) type Client = TestClient;

/// One route at `template` on the transport `config` describes, answered by
/// `handler` with the request's head and body set apart, as a handler written
/// on the vocabulary reads them.
pub(crate) async fn vocabulary_route_on<F, Fut>(
    config: HttpConfig,
    method: nest_rs_http::Method,
    template: &'static str,
    handler: F,
) -> Client
where
    F: Fn(nest_rs_http::Request, nest_rs_http::RequestBody) -> Fut + Copy + Send + Sync + 'static,
    Fut:
        std::future::Future<Output = nest_rs_http::Result<nest_rs_http::Response>> + Send + 'static,
{
    routes::table_on(config, move |routes| {
        routes.route(
            method.clone(),
            template,
            nest_rs_http::endpoint_fn(move |req: nest_rs_http::Request| {
                let (req, body) = req.split();
                handler(req, body)
            }),
        );
    })
    .await
    .unwrap_or_else(|refused| panic!("the route boots: {refused}"))
}

/// [`vocabulary_route_on`] on the default transport.
pub(crate) async fn vocabulary_route<F, Fut>(
    method: nest_rs_http::Method,
    template: &'static str,
    handler: F,
) -> Client
where
    F: Fn(nest_rs_http::Request, nest_rs_http::RequestBody) -> Fut + Copy + Send + Sync + 'static,
    Fut:
        std::future::Future<Output = nest_rs_http::Result<nest_rs_http::Response>> + Send + 'static,
{
    vocabulary_route_on(HttpConfig::default(), method, template, handler).await
}

/// What a client reads of an answer: its status, its `Content-Type` and its
/// body.
pub(crate) async fn answered(resp: nest_rs_testing::TestResponse) -> (u16, String, String) {
    let status = resp.0.status().as_u16();
    let content_type = resp
        .0
        .headers()
        .get(nest_rs_http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = resp.0.into_body().into_string().await.expect("a text body");
    (status, content_type, body)
}

/// The kit's client over the transport's endpoint.
pub(crate) fn client(endpoint: nest_rs_http::BoxEndpoint) -> TestClient {
    TestClient::new(endpoint)
}
