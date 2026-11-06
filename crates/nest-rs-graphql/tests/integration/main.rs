//! Integration tests mirroring `src/` — one binary, one module per concern.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod ambient;
mod context;
mod document_cost;
mod duplicate_operation;
mod endpoint;
mod federation;
mod field_spellings;
mod global_pipe;
mod guard;
mod layer_pool;
mod limits;
mod loader;
mod module;
mod null_propagation;
mod operation;
mod operation_units;
mod pipe;
mod redact;
mod resolver;
mod scope;
mod sdl_snapshot;
mod subscription;
mod unit;
mod view;
mod wire_enum;

use nest_rs_core::{Layer, UnitContext, async_trait, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_testing::CapturedSpan;

/// `span` records the outcome its line filed, and an `Error` status.
fn assert_span_records(span: &CapturedSpan, outcome: &str) {
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(outcome),
        "{:?}",
        span.fields
    );
    assert_eq!(
        span.field("otel.status_code").as_deref(),
        Some("error"),
        "{:?}",
        span.fields
    );
}

/// `span` records no error: what its caller earned fails nothing on it.
fn assert_span_records_no_error(span: &CapturedSpan) {
    assert_eq!(span.field("error.type"), None, "{:?}", span.fields);
    assert_eq!(span.field("otel.status_code"), None, "{:?}", span.fields);
}

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
    Denial::unavailable(None, "the identity store did not answer").with_source(ConnectionRefused)
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

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
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
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(store_down())
    }
}

impl GuardFor<nest_rs_graphql::GraphqlView> for FieldStoreDown {}

/// What `app`'s `/graphql` answers `query` with.
async fn graphql_answer(app: &nest_rs_testing::TestApp, query: &str) -> serde_json::Value {
    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize()
}
