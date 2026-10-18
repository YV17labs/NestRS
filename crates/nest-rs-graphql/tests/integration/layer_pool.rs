//! The layer-pool contract on the `/graphql` self-mount: a global guard runs
//! **exactly once** per request, the guards a module declares for the
//! endpoint **add to** the global pool, a guard in both running once, and a
//! unit-stage interceptor runs around the request.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{EndpointGuards, async_trait};
use nest_rs_testing::TestApp;
use tokio::sync::Mutex;

static GUARD_RUNS: AtomicUsize = AtomicUsize::new(0);
static INTERCEPTOR_RUNS: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counters() {
    GUARD_RUNS.store(0, Ordering::SeqCst);
    INTERCEPTOR_RUNS.store(0, Ordering::SeqCst);
}

/// Counts every `check_http` execution, then admits the request.
#[injectable]
#[derive(Default)]
struct CountingPassGuard;

impl Layer for CountingPassGuard {}

#[async_trait]
impl Guard for CountingPassGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        GUARD_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for CountingPassGuard {}

/// Counts every unit it runs around, then forwards.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct CountingInterceptor;

impl Layer for CountingInterceptor {}

#[async_trait]
impl Interceptor for CountingInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        INTERCEPTOR_RUNS.fetch_add(1, Ordering::SeqCst);
        next.run(cx).await
    }
}

#[resolver]
struct PingResolver;

#[operations]
impl PingResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [CountingPassGuard, CountingInterceptor, PingResolver],
)]
struct FallbackModule;

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        CountingPassGuard,
        PingResolver,
        EndpointGuards<GraphqlView, (CountingPassGuard,)>,
    ],
)]
struct DeclaredModule;

async fn post_ping(app: &TestApp) {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    assert_eq!(
        json.value()
            .object()
            .get("data")
            .object()
            .get("ping")
            .string(),
        "pong",
    );
}

#[tokio::test]
async fn global_guard_runs_exactly_once_per_graphql_request() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<FallbackModule>()
        .use_guards_global([guard::<CountingPassGuard>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        GUARD_RUNS.load(Ordering::SeqCst),
        1,
        "the global guard pool runs at the request and nowhere else — the double-run \
         must never come back",
    );
}

#[tokio::test]
async fn a_guard_both_pooled_and_declared_for_the_endpoint_runs_once() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<DeclaredModule>()
        .use_guards_global([guard::<CountingPassGuard>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        GUARD_RUNS.load(Ordering::SeqCst),
        1,
        "the endpoint's guards join the global pool — a guard in both runs once",
    );
}

#[tokio::test]
async fn a_guard_declared_for_the_endpoint_runs_without_a_pool() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::for_module::<DeclaredModule>()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(GUARD_RUNS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_unit_stage_interceptor_runs_around_the_graphql_post() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::for_module::<FallbackModule>()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        INTERCEPTOR_RUNS.load(Ordering::SeqCst),
        1,
        "a self-mount's request is a unit of the HTTP edge: the unit stage runs there",
    );
}
