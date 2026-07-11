//! WI-8 GraphQL bridge: a request-scoped provider reached from a resolver via
//! [`Scoped<T>`] is one instance per operation (shared across the operation's
//! fields) and a fresh instance per operation — end-to-end through a real boot
//! and two `/graphql` POSTs.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use nest_rs_core::{injectable, module};
use nest_rs_graphql::async_graphql::Context;
use nest_rs_graphql::{GraphqlModule, Scoped, resolver};
use nest_rs_testing::TestApp;

/// A singleton source of monotonic stamps — shared across every request, so a
/// per-request instance that pulls one stamp gets a value distinct from the
/// next request's instance.
#[injectable]
#[derive(Default)]
struct Ticker {
    next: AtomicU64,
}

impl Ticker {
    fn stamp(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }
}

/// Request-scoped: built once per operation and cached. `id()` stamps from the
/// singleton [`Ticker`] on first read and memoizes it, so within one operation
/// every field observing this instance reads the same id, while a fresh
/// operation builds a fresh `Probe` that stamps a new id. `stamp` is a
/// non-`#[inject]` field, so `#[injectable]` default-initializes it (an empty
/// `OnceLock`).
#[injectable(scope = request)]
struct Probe {
    #[inject]
    ticker: Arc<Ticker>,
    stamp: OnceLock<u64>,
}

impl Probe {
    fn id(&self) -> u64 {
        *self.stamp.get_or_init(|| self.ticker.stamp())
    }
}

#[resolver]
struct ProbeResolver;

#[resolver]
impl ProbeResolver {
    #[query]
    #[public]
    async fn first(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        Ok(Scoped::<Probe>::from_context(ctx)?.id().to_string())
    }

    #[query]
    #[public]
    async fn second(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        Ok(Scoped::<Probe>::from_context(ctx)?.id().to_string())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [Ticker, Probe, ProbeResolver],
)]
struct ScopeTestModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<ScopeTestModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

async fn query_field(app: &TestApp, field: &str) -> String {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": format!("{{ {field} }}") }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json()
        .await
        .value()
        .object()
        .get("data")
        .object()
        .get(field)
        .string()
        .to_owned()
}

#[tokio::test]
async fn two_fields_of_one_operation_share_one_request_scoped_instance() {
    let app = boot().await;

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ first second }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let data = json.value().object().get("data").object();
    let first = data.get("first").string();
    let second = data.get("second").string();

    // One `Probe` per operation: the second field observes the same instance
    // the first stamped, so it reads back the memoized id rather than pulling a
    // new stamp from the singleton ticker.
    assert_eq!(
        first, second,
        "both fields resolve the same request-scoped instance within one operation",
    );
}

#[tokio::test]
async fn a_fresh_operation_builds_a_new_request_scoped_instance() {
    let app = boot().await;

    let a = query_field(&app, "first").await;
    let b = query_field(&app, "first").await;

    // Two operations, two request scopes, two `Probe`s — each stamps its own id
    // from the shared singleton ticker.
    assert_ne!(
        a, b,
        "a request-scoped instance must not carry across operations",
    );
}
