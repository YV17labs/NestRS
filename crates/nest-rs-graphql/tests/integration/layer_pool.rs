//! The layer-pool contract on the `/graphql` self-mount: a global guard runs
//! **exactly once** per request, and a registered operation guard **adds to**
//! the global pool, which leaves it only the guards it covers.

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{BoxFuture, GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::async_trait;
use nest_rs_interceptors::{Interceptor, Next, interceptor};
use nest_rs_testing::TestApp;
use poem::{Request, Response};
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

/// Counts every HTTP interception, then forwards.
#[injectable]
#[derive(Default)]
struct CountingInterceptor;

impl Layer for CountingInterceptor {}

#[async_trait]
impl Interceptor for CountingInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> poem::Result<Response> {
        INTERCEPTOR_RUNS.fetch_add(1, Ordering::SeqCst);
        next.run(req).await
    }
}

/// A registered operation guard that runs nothing and covers nothing.
#[injectable]
#[derive(Default)]
struct NoopOpGuard;

impl GraphqlOperationGuard for NoopOpGuard {
    fn before<'a>(
        &'a self,
        _req: &'a mut nest_rs_http::Request,
    ) -> BoxFuture<'a, Result<(), nest_rs_http::Response>> {
        Box::pin(async move { Ok(()) })
    }

    fn around<'a>(
        &'a self,
        _req: &'a nest_rs_http::Request,
        inner: BoxFuture<'a, ()>,
    ) -> BoxFuture<'a, ()> {
        inner
    }
}

/// A registered operation guard claiming [`CountingPassGuard`] as its own, as
/// a bridge does the guards it runs.
#[injectable]
#[derive(Default)]
struct CoveringOpGuard;

impl GraphqlOperationGuard for CoveringOpGuard {
    fn before<'a>(
        &'a self,
        _req: &'a mut nest_rs_http::Request,
    ) -> BoxFuture<'a, Result<(), nest_rs_http::Response>> {
        Box::pin(async move { Ok(()) })
    }

    fn around<'a>(
        &'a self,
        _req: &'a nest_rs_http::Request,
        inner: BoxFuture<'a, ()>,
    ) -> BoxFuture<'a, ()> {
        inner
    }

    fn covers(&self) -> &[TypeId] {
        const COVERS: [TypeId; 1] = [TypeId::of::<CountingPassGuard>()];
        &COVERS
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
    providers = [CountingPassGuard, NoopOpGuard as dyn GraphqlOperationGuard, PingResolver],
)]
struct BridgedModule;

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [CountingPassGuard, CoveringOpGuard as dyn GraphqlOperationGuard, PingResolver],
)]
struct CoveringModule;

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
        "the global guard pool runs in-band (fallback operation guard) and nowhere else — \
         the edge double-run must never come back",
    );
}

#[tokio::test]
async fn a_registered_operation_guard_adds_to_the_pool() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<BridgedModule>()
        .use_guards_global([guard::<CountingPassGuard>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        GUARD_RUNS.load(Ordering::SeqCst),
        1,
        "a registered GraphqlOperationGuard joins the global pool — a pooled guard it does \
         not cover still runs, once",
    );
}

#[tokio::test]
async fn the_pool_leaves_a_covered_guard_to_the_operation_guard() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<CoveringModule>()
        .use_guards_global([guard::<CountingPassGuard>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        GUARD_RUNS.load(Ordering::SeqCst),
        0,
        "the guard the operation guard covers is its to run — the pool does not run it again",
    );
}

#[tokio::test]
async fn the_global_interceptor_pool_covers_the_graphql_post() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<FallbackModule>()
        .use_interceptors_global([interceptor::<CountingInterceptor>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        INTERCEPTOR_RUNS.load(Ordering::SeqCst),
        1,
        "the transport-edge interceptor pool wraps self-mounted surfaces too",
    );
}
