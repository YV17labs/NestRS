//! `/graphql` and `/mcp` answer the **same app wiring** the same way.
//!
//! Both are `EdgePosture::Exempt`: no guard runs at the HTTP edge, so each
//! gates in-band through its own operation-guard seam. That gives three
//! wirings, and this file pins all three against *one* module rather than
//! testing each transport in its own crate and hoping they agree:
//!
//! 1. **Only `use_guards_global(...)`** — both fold the global pool in-band
//!    (`FallbackOperationGuard` / `FallbackMcpGuard`).
//! 2. **A bridge registered** — the bridge owns the chain on that transport and
//!    the pool does not also run.
//! 3. **Neither** — the documented asymmetry, asserted rather than described.
//!
//! The one behavioural difference in wiring 1 is deliberate and is the reason
//! this file exists: `/graphql` carries the `Public` marker, so a pooled
//! authentication guard admits an anonymous operation through to the resolver
//! gates; `/mcp` carries none, so the same guard in the same pool refuses the
//! tool call. A regression that silently opened `/mcp` to anonymous callers
//! would pass every per-transport test and fail here.

use nest_rs_core::{HandlerMetadata, Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, resolver};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{Reflector, async_trait};
use nest_rs_mcp::{
    AllowAllMcpGuard, McpOperationGuard, ServerHandler, mcp, tool_handler, tool_router,
};
use nest_rs_testing::{TestApp, TestResponse, mcp::post_message};
use poem::Request;
use poem::http::StatusCode;

/// Stands in for `AuthnGuard`: requires a bearer unless the surface declared
/// itself public. Honouring `Public` is a *guard* policy, not a framework skip
/// — which is exactly what makes the two transports diverge here.
#[injectable]
#[derive(Default)]
struct BearerGuard;

impl Layer for BearerGuard {}

#[async_trait]
impl Guard for BearerGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        if req.headers().contains_key("authorization") || Reflector::new(req).is_public() {
            return Ok(());
        }
        Err(Denial::unauthorized("missing bearer token"))
    }
}

#[resolver]
struct ParityResolver;

#[resolver]
impl ParityResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[mcp(path = "/mcp")]
#[derive(Clone)]
struct ParityTool;

#[tool_router]
impl ParityTool {}

#[tool_handler]
impl ServerHandler for ParityTool {}

/// One module, both transports, **no** operation-guard bridge on either side.
#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [BearerGuard, ParityResolver, ParityTool],
)]
struct BothTransportsModule;

/// Same, plus an MCP bridge — the "a bridge is registered" wiring.
#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        BearerGuard,
        ParityResolver,
        ParityTool,
        AllowAllMcpGuard as dyn McpOperationGuard,
    ],
)]
struct BridgedMcpModule;

/// `initialize` against `/mcp`, returning the raw response — these tests assert
/// on the status the mount answers with, not on a completed session.
async fn mcp_initialize(app: &TestApp, bearer: Option<&str>) -> TestResponse {
    post_message(
        app.http(),
        "/mcp",
        None,
        bearer,
        &nest_rs_testing::mcp::initialize_request(),
    )
    .await
}

async fn pooled() -> TestApp {
    TestApp::builder()
        .module::<BothTransportsModule>()
        .use_guards_global([guard::<BearerGuard>()])
        .build()
        .await
        .expect("both transports boot under one global pool")
}

// Wiring 1, the headline: one `use_guards_global(...)` gates both transports
// in-band. Before `FallbackMcpGuard` existed, the identical call reached
// `/graphql` and left `/mcp` dead on 401 regardless of the token.
#[tokio::test]
async fn a_global_pool_gates_both_transports_for_an_authenticated_caller() {
    let app = pooled().await;

    let gql = app
        .http()
        .post("/graphql")
        .header("authorization", "Bearer t")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    gql.assert_status(StatusCode::OK);
    let body = gql.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("pong"),
        "the pooled guard admits an authenticated GraphQL operation: {body}",
    );

    let mcp = mcp_initialize(&app, Some("Bearer t")).await;
    mcp.assert_status(StatusCode::OK);
}

// The same pool refuses on both when the guard's own policy says so — the
// pooled guard runs in-band on `/mcp`, it is not merely absent.
#[tokio::test]
async fn the_pooled_guards_policy_decides_on_mcp_too() {
    let app = pooled().await;

    let denied = mcp_initialize(&app, None).await;
    denied.assert_status(StatusCode::UNAUTHORIZED);
}

// The deliberate asymmetry, pinned. `/graphql`'s `Public` marker lets the same
// pooled guard admit an anonymous operation through to the resolver gates;
// `/mcp` has no such marker and refuses. Flipping either half is a security
// change, and this is where it gets caught.
#[tokio::test]
async fn only_graphql_carries_the_public_marker_for_anonymous_callers() {
    let app = pooled().await;

    let gql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    gql.assert_status(StatusCode::OK);
    let body = gql.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("pong"),
        "anonymous GraphQL reaches the resolver gates (the `Public` marker): {body}",
    );

    let mcp = mcp_initialize(&app, None).await;
    mcp.assert_status(StatusCode::UNAUTHORIZED);
}

// Wiring 2: a registered bridge owns the chain on its transport — the global
// pool must not also run there (nothing runs twice), while the other transport
// keeps folding the pool.
#[tokio::test]
async fn a_registered_bridge_replaces_the_pool_on_its_transport_only() {
    let app = TestApp::builder()
        .module::<BridgedMcpModule>()
        .use_guards_global([guard::<BearerGuard>()])
        .build()
        .await
        .expect("boots");

    // `AllowAllMcpGuard` replaced the fallback: no bearer, still admitted.
    let mcp = mcp_initialize(&app, None).await;
    mcp.assert_status(StatusCode::OK);

    // GraphQL still has no bridge, so it still folds the pool.
    let gql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    gql.assert_status(StatusCode::OK);
}

// Wiring 3: neither bridge nor pool. This is the one place the two transports
// genuinely differ by design — GraphQL runs unguarded (with a boot `warn`),
// MCP refuses — because their no-guard defaults were chosen differently. Pinned
// so the difference stays a decision rather than an accident.
#[tokio::test]
async fn with_neither_bridge_nor_pool_graphql_runs_open_and_mcp_refuses() {
    let app = TestApp::for_module::<BothTransportsModule>()
        .await
        .expect("boots");

    let gql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    gql.assert_status(StatusCode::OK);
    let body = gql.0.into_body().into_string().await.expect("body");
    assert!(body.contains("pong"), "graphql has no gate at all: {body}");

    let mcp = mcp_initialize(&app, None).await;
    mcp.assert_status(StatusCode::UNAUTHORIZED);
}
