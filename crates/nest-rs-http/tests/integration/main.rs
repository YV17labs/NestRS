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
mod edge;
mod exclusive_paths;
mod fail_secure;
mod fallback;
mod global_prefix;
mod headers;
mod input;
mod opaque;
mod pipe;
mod problem;
mod response_body;
mod route_decorators;
mod security_headers;
mod server;
mod sse;
mod tls;
mod trace_context;
mod transport;
mod versioning;

use nest_rs_core::{App, Module, Transport};
use nest_rs_http::HttpTransport;
use poem::endpoint::BoxEndpoint;
use poem::test::TestClient;

/// Boot `M` through a real `HttpTransport` and hand back a client over the
/// composed endpoint; `nest-rs-testing`'s `TestApp` would be a dependency cycle.
pub(crate) async fn boot<M>() -> TestClient<BoxEndpoint<'static, poem::Response>>
where
    M: Module + 'static,
{
    boot_on::<M>(HttpTransport::new()).await
}

/// The same boot, on a transport the caller has already configured.
pub(crate) async fn boot_on<M>(
    mut transport: HttpTransport,
) -> TestClient<BoxEndpoint<'static, poem::Response>>
where
    M: Module + 'static,
{
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("module boots");
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    TestClient::new(
        transport
            .take_endpoint()
            .expect("configure populates the endpoint"),
    )
}
