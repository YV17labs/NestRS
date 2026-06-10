//! The layer-pool contract on the `/graphql` self-mount.
//!
//! `/graphql` is `EdgePosture::Exempt`: no guard runs at the HTTP edge. The
//! per-operation seam is the **only** guard site — the registered
//! `GraphqlOperationGuard` when the app binds one, otherwise the global-pool
//! fallback `use_guards_global` seeds. These tests pin the two halves of
//! that contract: a global guard runs **exactly once** per GraphQL request
//! (the historical double-run — edge + in-band — must never come back), and
//! a registered operation guard **replaces** the fallback (the real bridge
//! runs the same guards itself; a second site would double-run). The
//! transport-edge interceptor pool still covers the POST.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{BoxFuture, GraphqlModule, GraphqlOperationGuard, resolver};
use nest_rs_guards::{Denial, Guard, guard};
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
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        GUARD_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

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

/// A registered operation guard that runs nothing — stands in for an app
/// bridge that owns the chain itself.
#[injectable]
#[derive(Default)]
struct NoopOpGuard;

impl GraphqlOperationGuard for NoopOpGuard {
    fn before<'a>(&'a self, _req: &'a mut Request) -> BoxFuture<'a, Result<(), Response>> {
        Box::pin(async move { Ok(()) })
    }

    fn around<'a>(
        &'a self,
        _req: &'a Request,
        inner: BoxFuture<'a, Response>,
    ) -> BoxFuture<'a, Response> {
        inner
    }
}

#[resolver]
struct PingResolver;

#[resolver]
impl PingResolver {
    #[query]
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
async fn a_registered_operation_guard_replaces_the_fallback() {
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
        0,
        "a registered GraphqlOperationGuard owns the chain — the fallback must not also run \
         the global pool (the real bridge runs the same guards itself)",
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
