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
mod global_prefix;
mod guard_stage;
mod headers;
mod input;
mod opaque;
mod pipe;
mod problem;
mod response_body;
mod route_decorators;
mod route_shaper;
mod security_headers;
mod server;
mod sse;
mod tls;
mod trace_context;
mod transport;
mod versioning;

use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
use nest_rs_core::{App, Handler, Module, Posture, Transport};
use nest_rs_http::{HttpConfig, HttpTransport};
use poem::endpoint::BoxEndpoint;
use poem::test::TestClient;

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
pub(crate) async fn boot<M>() -> TestClient<BoxEndpoint<'static, poem::Response>>
where
    M: Module + 'static,
{
    boot_on::<M>(HttpConfig::default()).await
}

/// The transport `config` describes, before `configure`.
pub(crate) fn built(config: &HttpConfig) -> HttpTransport {
    HttpTransport::from_config(config).expect("the config is in range")
}

/// The same boot, on the transport `config` describes.
pub(crate) async fn boot_on<M>(
    config: HttpConfig,
) -> TestClient<BoxEndpoint<'static, poem::Response>>
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

/// poem's client over the transport's endpoint.
pub(crate) fn client(
    endpoint: nest_rs_http::BoxEndpoint,
) -> TestClient<BoxEndpoint<'static, poem::Response>> {
    TestClient::new(poem::EndpointExt::boxed(
        nest_rs_http::__private::poem_bridge::to_poem(endpoint),
    ))
}
