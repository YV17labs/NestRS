//! `nest-rs-resilience` in process: the documented wiring booted — one policy
//! per instance, imported bare or pinned — a refusal answered by every edge,
//! `#[resilient]`'s expansion run on each host it takes, and the middleware on
//! a reqwest client sending real requests to a TLS peer double whose
//! certificate a `TestAuthority` issued.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod config;
mod error;
mod middleware;
mod module;
mod policy;
mod resilient;

use std::sync::Arc;

use nest_rs_core::{Instance, module};
use nest_rs_resilience::{ResilienceConfig, ResilienceModule, ResiliencePolicy};
use nest_rs_testing::HeadlessApp;

/// No breaker and no retry: a row of a test sets what it is about.
fn quiet() -> ResilienceConfig {
    ResilienceConfig {
        retries: None,
        breaker_window: None,
        ..ResilienceConfig::default()
    }
}

/// The default instance pinned to `config`, as `for_root` pins it.
async fn policy(config: ResilienceConfig) -> (HeadlessApp, Arc<ResiliencePolicy>) {
    #[module(imports = [ResilienceModule])]
    struct Bare;

    let app = HeadlessApp::builder()
        .module::<Bare>()
        .provide(config)
        .build_headless()
        .await
        .expect("a policy boots");
    let policy = held::<nest_rs_core::Unnamed>(&app);
    (app, policy)
}

/// The policy of the instance `I` the booted `app` holds.
fn held<I: Instance>(app: &HeadlessApp) -> Arc<ResiliencePolicy<I>> {
    app.container()
        .get::<ResiliencePolicy<I>>()
        .expect("the module built the instance's policy")
}
