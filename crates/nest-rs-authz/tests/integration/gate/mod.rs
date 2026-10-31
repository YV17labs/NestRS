//! Covers `src/gate.rs` — one class gate on every edge, through each edge's
//! real decorator: the refusal a wider token would fix reads the same sentence,
//! code and scope list on HTTP, GraphQL, WS and MCP, each edge carrying them
//! where it carries a client's; a denial no `when` narrows is refused there
//! plainly, before any body runs.

mod crud;
mod graphql;
mod mcp;
mod ws;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authz::{Ability, AbilityBuilder, Action, Create, Read};
use nest_rs_core::{Layer, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::StatusCode;
use nest_rs_http::{HttpView, controller, routes};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool_as;
use nest_rs_ws::{WsError, gateway, messages};

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

/// CASL's canonical policy, on a credential that delegated no scope: an
/// org-wide `Manage` with its `Create` clawed back, beside a `Create` grant the
/// token's scope would have unlocked, which no scope lifts past the denial.
fn denied_outright() -> Ability {
    let mut builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
    builder
        .can(Action::Manage, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 1));
    builder
        .can(Action::Create, widget::Entity)
        .requires_scope("widgets:write");
    builder.cannot(Action::Create, widget::Entity);
    builder.build().expect("valid test ability")
}

/// The bodies a create reached, on any edge: a refused caller reaches none.
static CREATED: AtomicUsize = AtomicUsize::new(0);

fn create_widget() -> i32 {
    CREATED.fetch_add(1, Ordering::SeqCst);
    1
}

/// Attaches [`too_narrow`] to every request it checks, naming its caller as
/// the authentication guard does.
#[injectable]
#[derive(Default)]
struct NarrowToken;

impl Layer for NarrowToken {}

#[async_trait]
impl Guard for NarrowToken {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("narrow");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(too_narrow()));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for NarrowToken {}

/// Attaches [`denied_outright`] to every request it checks.
#[injectable]
#[derive(Default)]
struct DeniedToken;

impl Layer for DeniedToken {}

#[async_trait]
impl Guard for DeniedToken {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("denied");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            denied_outright(),
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for DeniedToken {}

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

    #[post("/")]
    #[authorize(Create, widget::Entity)]
    async fn create(&self) -> String {
        create_widget().to_string()
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

    #[mutation]
    #[authorize(Create, widget::Entity)]
    async fn create_widget(&self) -> GqlResult<i32> {
        Ok(create_widget())
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

    /// Create a widget.
    #[tool]
    #[authorize(Create, widget::Entity)]
    async fn create_widget(&self) -> Result<String, McpError> {
        Ok(create_widget().to_string())
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

    #[subscribe_message("create")]
    #[authorize(Create, widget::Entity)]
    async fn create(&self) -> Result<String, WsError> {
        Ok(create_widget().to_string())
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [NarrowToken, WidgetsController, WidgetsResolver, WidgetsTool],
)]
struct EveryEdgeModule;

#[module(imports = [WsEdgeModule], providers = [GlobalGuards<(NarrowToken,)>])]
struct NarrowTokenWsApp;

#[module(imports = [EveryEdgeModule], providers = [GlobalGuards<(NarrowToken,)>])]
struct NarrowTokenEveryEdgeApp;

#[module(
    imports = [EveryEdgeModule],
    providers = [DeniedToken, GlobalGuards<(DeniedToken,)>],
)]
struct DeniedTokenEveryEdgeApp;

#[module(
    imports = [WsEdgeModule],
    providers = [DeniedToken, GlobalGuards<(DeniedToken,)>],
)]
struct DeniedTokenWsApp;

#[tokio::test]
async fn a_token_too_narrow_reads_one_sentence_one_code_and_one_scope_list_on_every_edge() {
    let app = TestApp::builder()
        .module::<NarrowTokenEveryEdgeApp>()
        .build()
        .await
        .expect("every edge boots");

    let http = app.http().get("/widgets").send().await;
    http.assert_status(StatusCode::FORBIDDEN);
    let challenge = http
        .0
        .headers()
        .get(nest_rs_http::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let problem: serde_json::Value =
        serde_json::from_str(&http.0.into_body().into_string().await.expect("a body"))
            .expect("a problem document");
    assert_eq!(problem["detail"], TOO_NARROW, "HTTP: {problem}");
    assert_eq!(problem["code"], "INSUFFICIENT_SCOPE", "HTTP: {problem}");
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

    let ws = TestApp::builder()
        .module::<NarrowTokenWsApp>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let frame = crate::ws::message_as(&ws, "/ws/widgets", None, "read").await;
    ws.shutdown().await.expect("the transport stops cleanly");
    let refused = &frame["data"];
    assert_eq!(refused["error"], TOO_NARROW, "WS: {frame}");
    assert_eq!(refused["code"], "INSUFFICIENT_SCOPE", "WS: {frame}");
    assert_eq!(
        refused["requiredScopes"],
        serde_json::json!(NEEDED),
        "WS: {frame}",
    );
}

/// The WebSocket edge, on a socket of its own: the pool attaches the token at
/// the upgrade, and the message's gate reads it.
#[module(imports = [nest_rs_ws::WsModule], providers = [NarrowToken, WidgetsGateway])]
struct WsEdgeModule;

#[tokio::test]
async fn a_denial_no_when_narrows_is_refused_plainly_on_every_edge_and_runs_no_body() {
    let app = TestApp::builder()
        .module::<DeniedTokenEveryEdgeApp>()
        .build()
        .await
        .expect("every edge boots");

    let http = app.http().post("/widgets").send().await;
    http.assert_status(StatusCode::FORBIDDEN);
    assert!(
        http.0
            .headers()
            .get(nest_rs_http::header::WWW_AUTHENTICATE)
            .is_none(),
        "HTTP: no challenge sends the client after a token refused all the same",
    );
    let problem: serde_json::Value =
        serde_json::from_str(&http.0.into_body().into_string().await.expect("a body"))
            .expect("a problem document");
    assert_eq!(problem["code"], "FORBIDDEN", "HTTP: {problem}");

    let graphql: serde_json::Value = serde_json::to_value(
        app.http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "mutation { createWidget }" }))
            .send()
            .await
            .json()
            .await,
    )
    .expect("a GraphQL response");
    let refused = &graphql["errors"][0];
    assert_eq!(
        refused["extensions"]["code"], "FORBIDDEN",
        "GraphQL: {graphql}"
    );
    assert!(
        refused["extensions"].get("requiredScopes").is_none(),
        "GraphQL: {graphql}"
    );

    let mcp = call_tool_as(
        app.http(),
        "/mcp",
        "create_widget",
        None,
        &[],
        serde_json::json!({}),
    )
    .await;
    assert!(mcp.contains("\"code\":\"FORBIDDEN\""), "MCP: {mcp}");
    assert!(!mcp.contains("requiredScopes"), "MCP: {mcp}");

    let ws = TestApp::builder()
        .module::<DeniedTokenWsApp>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let frame = crate::ws::message_as(&ws, "/ws/widgets", None, "create").await;
    ws.shutdown().await.expect("the transport stops cleanly");
    assert_eq!(frame["data"]["code"], "FORBIDDEN", "WS: {frame}");
    assert!(frame["data"].get("requiredScopes").is_none(), "WS: {frame}");

    assert_eq!(
        CREATED.load(Ordering::SeqCst),
        0,
        "a create body ran for a caller its policy denies outright",
    );
}
