//! Integration tests mirroring `src/` — one binary, one module per concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod context;
mod duplicate_operation;
mod federation;
mod global_pipe;
mod guard;
mod layer_pool;
mod limits;
mod loader;
mod module;
mod operation;
mod pipe;
mod read_only;
mod redact;
mod resolver;
mod scope;
mod sdl_snapshot;
mod subscription;
mod unit;
mod view;

use nest_rs_core::{Layer, UnitContext, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};

/// What a store refusing the connection reports: the cause a guard keeps.
#[derive(Debug)]
struct ConnectionRefused;

impl std::fmt::Display for ConnectionRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the identity store refused the connection")
    }
}

impl std::error::Error for ConnectionRefused {}

/// The `error` field a denial by either guard below files: its reason, then
/// its cause.
const STORE_DOWN_CHAIN: &str =
    "the identity store did not answer: the identity store refused the connection";

/// The denial a guard whose identity store is down answers, keeping why.
fn store_down() -> Denial {
    Denial::unavailable_because(None, "the identity store did not answer", ConnectionRefused)
}

/// Cannot evaluate the `/graphql` request: its identity store is down.
#[injectable]
#[derive(Default)]
struct RequestStoreDown;

impl Layer for RequestStoreDown {}

#[async_trait]
impl Guard for RequestStoreDown {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(store_down())
    }
}

impl GuardFor<nest_rs_http::HttpView> for RequestStoreDown {}

/// Cannot evaluate a field: its identity store is down.
#[injectable]
#[derive(Default)]
struct FieldStoreDown;

impl Layer for FieldStoreDown {}

#[async_trait]
impl Guard for FieldStoreDown {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(store_down())
    }
}

impl GuardFor<nest_rs_graphql::GraphqlView> for FieldStoreDown {}
