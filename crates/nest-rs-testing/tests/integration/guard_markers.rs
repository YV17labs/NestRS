//! A global guard overriding a `check_*` runs it at that edge whether or not it
//! declares the edge's marker trait (`HttpGuard`, …), which only the
//! decorators bound. WS needs a socket and is held in `nest-rs-ws`'s suite.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlOperationContext, operations, resolver};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_mcp::{McpError, McpOperationContext, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool;
use poem::Request;
use poem::http::StatusCode;

#[injectable]
#[derive(Default)]
struct UnmarkedHttpGuard;

impl Layer for UnmarkedHttpGuard {}

#[async_trait]
impl Guard for UnmarkedHttpGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::forbidden("refused by an unmarked HTTP check"))
    }
}

#[controller(path = "/unmarked")]
struct UnmarkedController;

#[routes]
impl UnmarkedController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "reached"
    }
}

#[module(providers = [UnmarkedHttpGuard, UnmarkedController])]
struct UnmarkedHttpModule;

#[tokio::test]
async fn a_pooled_guard_without_http_guard_still_checks_every_request() {
    let app = TestApp::builder()
        .module::<UnmarkedHttpModule>()
        .use_guards_global([guard::<UnmarkedHttpGuard>()])
        .build()
        .await
        .expect("an unmarked global guard boots");

    let response = app.http().get("/unmarked").send().await;
    response.assert_status(StatusCode::FORBIDDEN);
    let body = response.0.into_body().into_string().await.expect("a body");
    assert!(
        body.contains("refused by an unmarked HTTP check"),
        "the pool ran the guard's own check_http: {body}",
    );
}

#[injectable]
#[derive(Default)]
struct UnmarkedGraphqlGuard;

impl Layer for UnmarkedGraphqlGuard {}

#[async_trait]
impl Guard for UnmarkedGraphqlGuard {
    async fn check_graphql(&self, _op: &GraphqlOperationContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("refused by an unmarked GraphQL check"))
    }
}

#[resolver]
struct UnmarkedResolver;

#[operations]
impl UnmarkedResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [UnmarkedGraphqlGuard, UnmarkedResolver],
)]
struct UnmarkedGraphqlModule;

#[tokio::test]
async fn a_pooled_guard_without_graphql_guard_still_checks_every_operation() {
    let app = TestApp::builder()
        .module::<UnmarkedGraphqlModule>()
        .use_guards_global([guard::<UnmarkedGraphqlGuard>()])
        .build()
        .await
        .expect("an unmarked global guard boots");

    let response = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    let body = response.0.into_body().into_string().await.expect("a body");
    assert!(
        !body.contains("pong"),
        "the operation must not answer a caller its pooled guard refused: {body}",
    );
    assert!(
        body.contains("refused by an unmarked GraphQL check"),
        "the pool ran the guard's own check_graphql: {body}",
    );
}

#[injectable]
#[derive(Default)]
struct UnmarkedMcpGuard;

impl Layer for UnmarkedMcpGuard {}

#[async_trait]
impl Guard for UnmarkedMcpGuard {
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("refused by an unmarked MCP check"))
    }
}

#[mcp(path = "/mcp/unmarked")]
#[derive(Clone, Default)]
struct UnmarkedTool;

#[tools]
impl UnmarkedTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [UnmarkedMcpGuard, UnmarkedTool])]
struct UnmarkedMcpModule;

#[tokio::test]
async fn a_pooled_guard_without_mcp_guard_still_checks_every_operation() {
    let app = TestApp::builder()
        .module::<UnmarkedMcpModule>()
        .use_guards_global([guard::<UnmarkedMcpGuard>()])
        .build()
        .await
        .expect("an unmarked global guard boots");

    let body = call_tool(app.http(), "/mcp/unmarked", "ping", None).await;
    assert!(
        !body.contains("pong"),
        "the tool must not answer a caller its pooled guard refused: {body}",
    );
    assert!(
        body.contains("refused by an unmarked MCP check"),
        "the pool ran the guard's own check_mcp: {body}",
    );
}
