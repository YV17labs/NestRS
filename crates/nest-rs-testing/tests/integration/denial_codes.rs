//! One denial, one code, on every edge: a guard's refusal names its class by
//! the code a client branches on — the problem document's `code` on HTTP,
//! `extensions.code` on GraphQL, `errors.code` in a WebSocket frame, `data.code`
//! in an MCP error — the same one wherever it is read, an internal denial's
//! included.

use nest_rs_core::problem::code;
use nest_rs_core::{Code, Layer, UnitContext, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{HttpView, async_trait, controller, routes};
use nest_rs_mcp::{McpError, McpView, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_tool_statelessly, result};
use nest_rs_ws::{WsModule, WsView, gateway, messages};

/// Every denial a guard answers, by the operation it refuses, and the class
/// it is known by.
fn denials() -> [(&'static str, Denial, Code); 7] {
    [
        (
            "unauthorized",
            Denial::unauthorized("no credential"),
            code::UNAUTHENTICATED,
        ),
        (
            "invalid_credential",
            Denial::invalid_credential("the token expired", "invalid_token"),
            code::UNAUTHENTICATED,
        ),
        ("forbidden", Denial::forbidden("not yours"), code::FORBIDDEN),
        (
            "insufficient_scope",
            Denial::insufficient_scope(["posts:write"], "too narrow"),
            code::INSUFFICIENT_SCOPE,
        ),
        (
            "rate_limited",
            Denial::rate_limited(30, "slow down"),
            code::RATE_LIMITED,
        ),
        (
            "unavailable",
            Denial::unavailable(Some(7), "the store did not answer"),
            code::UNAVAILABLE,
        ),
        (
            "internal",
            Denial::internal("the store at 10.0.0.1 refused"),
            code::INTERNAL,
        ),
    ]
}

/// `operation` as GraphQL names its field.
fn field(operation: &str) -> String {
    operation
        .split('_')
        .enumerate()
        .map(|(at, word)| match at {
            0 => word.to_owned(),
            _ => word[..1].to_uppercase() + &word[1..],
        })
        .collect()
}

/// The denial an operation declared by `method` is refused with; any other
/// unit — a self-mount's request, a socket's upgrade — is admitted.
fn refusing(cx: &UnitContext<'_>) -> Result<(), Denial> {
    let method = cx.handler().method();
    match denials().into_iter().find(|(named, ..)| *named == method) {
        Some((_, denial, _)) => Err(denial),
        None => Ok(()),
    }
}

/// Refuses each operation with the denial its name says, on every edge.
#[injectable]
#[derive(Default)]
struct Refusing;

impl Layer for Refusing {}

#[async_trait]
impl Guard for Refusing {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        refusing(cx)
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        refusing(cx)
    }
}

impl GuardFor<HttpView> for Refusing {}
impl GuardFor<GraphqlView> for Refusing {}
impl GuardFor<McpView> for Refusing {}
impl GuardFor<WsView> for Refusing {}

#[controller(path = "/refused")]
#[derive(Default)]
struct RefusedController;

#[routes]
impl RefusedController {
    #[get("/unauthorized")]
    #[public]
    async fn unauthorized(&self) -> &'static str {
        "served"
    }

    #[get("/invalid_credential")]
    #[public]
    async fn invalid_credential(&self) -> &'static str {
        "served"
    }

    #[get("/forbidden")]
    #[public]
    async fn forbidden(&self) -> &'static str {
        "served"
    }

    #[get("/insufficient_scope")]
    #[public]
    async fn insufficient_scope(&self) -> &'static str {
        "served"
    }

    #[get("/rate_limited")]
    #[public]
    async fn rate_limited(&self) -> &'static str {
        "served"
    }

    #[get("/unavailable")]
    #[public]
    async fn unavailable(&self) -> &'static str {
        "served"
    }

    #[get("/internal")]
    #[public]
    async fn internal(&self) -> &'static str {
        "served"
    }
}

#[resolver]
struct RefusedResolver;

#[operations]
impl RefusedResolver {
    #[query]
    #[public]
    async fn unauthorized(&self) -> GqlResult<String> {
        Ok("served".into())
    }

    #[query]
    #[public]
    async fn invalid_credential(&self) -> GqlResult<String> {
        Ok("served".into())
    }

    #[query]
    #[public]
    async fn forbidden(&self) -> GqlResult<String> {
        Ok("served".into())
    }

    #[query]
    #[public]
    async fn insufficient_scope(&self) -> GqlResult<String> {
        Ok("served".into())
    }

    #[query]
    #[public]
    async fn rate_limited(&self) -> GqlResult<String> {
        Ok("served".into())
    }

    #[query]
    #[public]
    async fn unavailable(&self) -> GqlResult<String> {
        Ok("served".into())
    }

    #[query]
    #[public]
    async fn internal(&self) -> GqlResult<String> {
        Ok("served".into())
    }
}

#[mcp]
#[derive(Clone, Default)]
struct RefusedTools;

#[tools]
impl RefusedTools {
    /// Refused unauthenticated.
    #[tool]
    #[public]
    async fn unauthorized(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }

    /// Refused for its credential.
    #[tool]
    #[public]
    async fn invalid_credential(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }

    /// Refused outright.
    #[tool]
    #[public]
    async fn forbidden(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }

    /// Refused for its scope.
    #[tool]
    #[public]
    async fn insufficient_scope(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }

    /// Refused for its rate.
    #[tool]
    #[public]
    async fn rate_limited(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }

    /// Refused for a dependency.
    #[tool]
    #[public]
    async fn unavailable(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }

    /// Refused by a failure.
    #[tool]
    #[public]
    async fn internal(&self) -> Result<String, McpError> {
        Ok("served".to_owned())
    }
}

#[gateway(path = "/ws/refused")]
struct RefusedGateway;

#[messages]
impl RefusedGateway {
    #[subscribe_message("unauthorized")]
    #[public]
    async fn unauthorized(&self) -> &'static str {
        "served"
    }

    #[subscribe_message("invalid_credential")]
    #[public]
    async fn invalid_credential(&self) -> &'static str {
        "served"
    }

    #[subscribe_message("forbidden")]
    #[public]
    async fn forbidden(&self) -> &'static str {
        "served"
    }

    #[subscribe_message("insufficient_scope")]
    #[public]
    async fn insufficient_scope(&self) -> &'static str {
        "served"
    }

    #[subscribe_message("rate_limited")]
    #[public]
    async fn rate_limited(&self) -> &'static str {
        "served"
    }

    #[subscribe_message("unavailable")]
    #[public]
    async fn unavailable(&self) -> &'static str {
        "served"
    }

    #[subscribe_message("internal")]
    #[public]
    async fn internal(&self) -> &'static str {
        "served"
    }
}

#[module(
    imports = [GraphqlModule::for_root(None), WsModule],
    providers = [Refusing, RefusedController, RefusedResolver, RefusedTools, RefusedGateway],
)]
struct EveryEdgeModule;

#[tokio::test]
async fn every_denial_names_one_code_on_every_edge() {
    let app = TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([guard::<Refusing>()])
        .build()
        .await
        .expect("every edge boots");
    let ws = TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([guard::<Refusing>()])
        .build_ws()
        .await
        .expect("every edge serves");
    let mut socket = ws.socket("/ws/refused").connect().await;

    for (operation, denial, class) in denials() {
        let refused = app.http().get(format!("/refused/{operation}")).send().await;
        assert_eq!(
            refused.0.status().as_u16(),
            denial.http_status(),
            "{operation}"
        );
        let http: serde_json::Value = refused.json().await.value().deserialize();

        let graphql: serde_json::Value = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": format!("{{ {} }}", field(operation)) }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize();

        let mcp = result(
            &call_tool_statelessly(
                app.http(),
                "/mcp",
                operation,
                None,
                &[],
                serde_json::json!({}),
            )
            .await,
        );

        socket.send(operation, serde_json::Value::Null).await;
        let frame = socket.next_envelope().await;

        let said = [
            ("HTTP", &http["code"]),
            ("GraphQL", &graphql["errors"][0]["extensions"]["code"]),
            ("MCP", &mcp["error"]["data"]["code"]),
            ("WS", &frame["data"]["errors"]["code"]),
        ];
        for (edge, code) in said {
            assert_eq!(
                code,
                class.as_str(),
                "{operation} on {edge}: HTTP {http}, GraphQL {graphql}, MCP {mcp}, WS {frame}",
            );
        }
        let answers = format!("{http} {graphql} {mcp} {frame}");
        assert!(!answers.contains("10.0.0.1"), "{operation}: {answers}");
    }
    drop(socket);
    ws.close().await;
}
