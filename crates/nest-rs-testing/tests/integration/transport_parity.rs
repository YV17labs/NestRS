//! `/graphql` and `/mcp` answer the **same app wiring** the same way, gating
//! in-band under three wirings: a global pool alone, a bridge registered, and
//! neither.
//!
//! The deliberate difference: `/graphql` carries the `Public` marker, so a
//! pooled authentication guard admits an anonymous operation; `/mcp` carries
//! none and refuses.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::HandlerMetadata;
use nest_rs_http::{Reflector, async_trait};
use nest_rs_mcp::{
    AllowAllMcpGuard, McpOperationGuard, ServerHandler, mcp, tool_handler, tool_router,
};
// The name rmcp's `#[tool_router]` / `#[tool_handler]` expansions resolve.
use nest_rs_mcp::rmcp;
use nest_rs_testing::{TestApp, TestResponse, mcp::post_message};
use poem::Request;
use poem::http::StatusCode;

/// Stands in for `AuthnGuard`: requires a bearer unless the surface declared
/// itself public.
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

impl HttpGuard for BearerGuard {}

#[resolver]
struct ParityResolver;

#[operations]
impl ParityResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[mcp]
#[derive(Clone)]
struct ParityTool;

#[tool_router(allow_empty)]
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

/// `initialize` against `/mcp`, returning the raw response.
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

#[tokio::test]
async fn the_pooled_guards_policy_decides_on_mcp_too() {
    let app = pooled().await;

    let denied = mcp_initialize(&app, None).await;
    denied.assert_status(StatusCode::UNAUTHORIZED);
}

// Flipping either half is a security change.
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

#[tokio::test]
async fn a_registered_bridge_replaces_the_pool_on_its_transport_only() {
    let app = TestApp::builder()
        .module::<BridgedMcpModule>()
        .use_guards_global([guard::<BearerGuard>()])
        .build()
        .await
        .expect("boots");

    let mcp = mcp_initialize(&app, None).await;
    mcp.assert_status(StatusCode::OK);

    let gql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    gql.assert_status(StatusCode::OK);
}

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
