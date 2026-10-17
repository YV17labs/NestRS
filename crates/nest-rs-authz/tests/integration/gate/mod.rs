//! Covers `src/gate.rs` — one class gate on every edge, through each edge's
//! real decorator: the refusal a wider token would fix reads the same sentence,
//! code and scope list on HTTP, GraphQL, WS and MCP, each edge carrying them
//! where it carries a client's.

mod graphql;
mod mcp;
mod ws;

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{HttpView, async_trait, controller, routes};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool_as;
use nest_rs_ws::{WsError, WsReply, gateway, messages};
use poem::http::StatusCode;

use crate::widget;

/// RFC 6750 §3.1's description of `insufficient_scope`, the one sentence.
const TOO_NARROW: &str = "the request requires higher privileges than provided by the access token";

/// A credential that delegated no scope, beside two grants that would each
/// allow a read: the refusal a wider token would have fixed. The read's own
/// grant is the narrowest, so its scope is the one every edge names.
fn too_narrow() -> Ability {
    let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
    builder
        .can(Action::Read, widget::Entity)
        .requires_scope("widgets:read");
    builder
        .can(Action::Manage, widget::Entity)
        .requires_scope("widgets:admin");
    builder.build().expect("valid test ability")
}

/// The one list every edge names: what the narrowest grant lacks.
const NEEDED: [&str; 1] = ["widgets:read"];

/// Attaches [`too_narrow`] to every request it checks.
#[injectable]
#[derive(Default)]
struct NarrowToken;

impl Layer for NarrowToken {}

#[async_trait]
impl Guard for NarrowToken {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(too_narrow());
        Ok(())
    }
}

impl GuardFor<HttpView> for NarrowToken {}

#[controller(path = "/widgets")]
#[derive(Default)]
struct WidgetsController;

#[routes]
impl WidgetsController {
    #[get("/")]
    #[authorize(Read, widget::Entity)]
    async fn list(&self) -> String {
        "widgets".to_owned()
    }
}

#[derive(SimpleObject, serde::Serialize, serde::Deserialize)]
struct WidgetRow {
    id: i32,
}

#[resolver]
struct WidgetsResolver;

#[operations]
impl WidgetsResolver {
    #[query]
    #[authorize(Read, widget::Entity)]
    async fn widgets(&self) -> GqlResult<Vec<WidgetRow>> {
        Ok(Vec::new())
    }
}

#[mcp(path = "/mcp")]
#[derive(Clone, Default)]
struct WidgetsTool;

#[tools]
impl WidgetsTool {
    /// List the widgets.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn read_widgets(&self) -> Result<String, McpError> {
        Ok("widgets".to_owned())
    }
}

#[gateway(path = "/ws/widgets")]
#[derive(Default)]
struct WidgetsGateway;

#[messages]
impl WidgetsGateway {
    #[subscribe_message("read")]
    #[authorize(Read, widget::Entity)]
    async fn read(&self) -> Result<String, WsError> {
        Ok("widgets".to_owned())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [NarrowToken, WidgetsController, WidgetsResolver, WidgetsTool],
)]
struct EveryEdgeModule;

#[tokio::test]
async fn a_token_too_narrow_reads_one_sentence_one_code_and_one_scope_list_on_every_edge() {
    let app = TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([guard::<NarrowToken>()])
        .build()
        .await
        .expect("every edge boots");

    let http = app.http().get("/widgets").send().await;
    http.assert_status(StatusCode::FORBIDDEN);
    let challenge = http
        .0
        .headers()
        .get(poem::http::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let problem: serde_json::Value =
        serde_json::from_str(&http.0.into_body().into_string().await.expect("a body"))
            .expect("a problem document");
    assert_eq!(problem["detail"], TOO_NARROW, "HTTP: {problem}");
    assert_eq!(
        challenge, r#"Bearer error="insufficient_scope", scope="widgets:read""#,
        "HTTP carries the code and RFC 6750's `scope` on its challenge",
    );

    let graphql: serde_json::Value = serde_json::to_value(
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ widgets { id } }" }))
            .send()
            .await
            .json()
            .await,
    )
    .expect("a GraphQL response");
    let refused = &graphql["errors"][0];
    assert_eq!(refused["message"], TOO_NARROW, "GraphQL: {graphql}");
    assert_eq!(refused["extensions"]["code"], "INSUFFICIENT_SCOPE");
    assert_eq!(
        refused["extensions"]["requiredScopes"],
        serde_json::json!(NEEDED),
        "GraphQL: {graphql}",
    );

    let mcp = call_tool_as(
        app.http(),
        "/mcp",
        "read_widgets",
        None,
        &[],
        serde_json::json!({}),
    )
    .await;
    assert!(mcp.contains(TOO_NARROW), "MCP: {mcp}");
    assert!(
        mcp.contains("\"code\":\"INSUFFICIENT_SCOPE\""),
        "MCP: {mcp}"
    );
    assert!(
        mcp.contains(r#""requiredScopes":["widgets:read"]"#),
        "MCP: {mcp}"
    );

    let ws = crate::ws::dispatch_with(&WidgetsGateway, Arc::new(too_narrow()), "read").await;
    let WsReply::Error(refused) = ws else {
        panic!("WS: the gate refuses the message");
    };
    assert_eq!(refused.error, TOO_NARROW, "WS: {refused:?}");
    assert_eq!(
        refused.errors.as_ref().map(|errors| &errors["code"]),
        Some(&serde_json::json!("INSUFFICIENT_SCOPE")),
        "WS: {refused:?}",
    );
    assert_eq!(
        refused
            .errors
            .as_ref()
            .map(|errors| &errors["requiredScopes"]),
        Some(&serde_json::json!(NEEDED)),
        "WS: {refused:?}",
    );
}
