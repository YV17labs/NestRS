//! Integration tests mirroring `src/` (see CLAUDE.md) — one binary, one module per concern.

mod body_limit;
mod compression;
mod controller;
mod diagnostics;
mod edge;
mod exclusive_paths;
mod fail_secure;
mod global_prefix;
mod header;
mod input;
mod response_body;
mod route_decorators;
mod security_headers;
mod sse;
mod tls;
mod trace_context;
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
    boot_on::<M>(HttpTransport::new()).await
}

/// The same boot, on a transport the caller has already configured by hand.
///
/// Half this suite pins one builder call — a body cap, a compression mode, a
/// global prefix, a `SecurityHeadersConfig` — and every one of them had copied
/// the six lines around it, `expect` strings included. The transport is the only
/// thing that ever differed, so it is the only thing a caller passes.
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
