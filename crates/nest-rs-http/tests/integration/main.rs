//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.

mod body_limit;
mod compression;
mod controller;
mod diagnostics;
mod edge;
mod exclusive_paths;
mod fail_secure;
mod global_prefix;
mod input;
mod route_decorators;
mod versioning;

use nest_rs_core::{App, Module, Transport};
use nest_rs_http::HttpTransport;
use poem::endpoint::BoxEndpoint;
use poem::test::TestClient;

/// Boot `M` through a real `HttpTransport` and hand back a client over the
/// composed endpoint — the arrange every module in this suite shares, so a
/// change to the transport's setup is one edit rather than one per file.
///
/// `nest-rs-testing`'s `TestApp` is the same thing one layer up; this crate is
/// the one that cannot reach for it without a dependency cycle, so the harness
/// lives here instead.
pub(crate) async fn boot<M>() -> TestClient<BoxEndpoint<'static, poem::Response>>
where
    M: Module + 'static,
{
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new();
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
