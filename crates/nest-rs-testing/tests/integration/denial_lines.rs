//! One denial files one line, on every edge: the line of the guard that decided
//! it when the guard files its own (`Guard::files_its_denials`), else the edge's
//! `guard denied …` on `nest_rs::layers`, so a guard that says nothing is still
//! seen.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{HttpView, async_trait, controller, routes};
use nest_rs_mcp::{McpError, McpView, mcp, tools};
use nest_rs_testing::mcp::{call_tool_as, initialize_request, post_message};
use nest_rs_testing::{CapturedEvent, LogCapture, TestApp};
use nest_rs_ws::{WsModule, WsView, gateway, messages};
use poem::http::StatusCode;

/// What a guard filing its own line files for each denial.
const FILED: &str = "the guard refused and said so";

/// A guard refusing every request, filing [`FILED`] first.
#[injectable]
#[derive(Default)]
struct FilingDoor;

impl Layer for FilingDoor {}

#[async_trait]
impl Guard for FilingDoor {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        tracing::warn!(target: "acme::door", door = "front", "the guard refused and said so");
        Err(Denial::forbidden("closed"))
    }

    fn files_its_denials(&self) -> bool {
        true
    }
}

impl GuardFor<HttpView> for FilingDoor {}

/// A guard refusing every request, saying nothing.
#[injectable]
#[derive(Default)]
struct SilentDoor;

impl Layer for SilentDoor {}

#[async_trait]
impl Guard for SilentDoor {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed"))
    }
}

impl GuardFor<HttpView> for SilentDoor {}

/// A guard admitting every request and refusing every unit it carries, filing
/// [`FILED`] first.
#[injectable]
#[derive(Default)]
struct FilingField;

impl Layer for FilingField {}

#[async_trait]
impl Guard for FilingField {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        tracing::warn!(target: "acme::field", field = "any", "the guard refused and said so");
        Err(Denial::forbidden("closed"))
    }

    fn files_its_denials(&self) -> bool {
        true
    }
}

/// A guard admitting every request and refusing every unit it carries, saying
/// nothing.
#[injectable]
#[derive(Default)]
struct SilentField;

impl Layer for SilentField {}

#[async_trait]
impl Guard for SilentField {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed"))
    }
}

impl GuardFor<GraphqlView> for FilingField {}
impl GuardFor<McpView> for FilingField {}
impl GuardFor<WsView> for FilingField {}
impl GuardFor<GraphqlView> for SilentField {}
impl GuardFor<McpView> for SilentField {}
impl GuardFor<WsView> for SilentField {}

#[controller(path = "/door")]
#[derive(Default)]
struct DoorController;

#[routes]
impl DoorController {
    #[get("/")]
    #[public]
    async fn open(&self) -> &'static str {
        "open"
    }
}

#[resolver]
struct DoorResolver;

#[operations]
impl DoorResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[mcp]
#[derive(Clone, Default)]
struct DoorTool;

#[tools]
impl DoorTool {
    /// Answer.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[gateway(path = "/ws/door")]
struct DoorGateway;

#[messages]
impl DoorGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(
    imports = [GraphqlModule::for_root(None), WsModule],
    providers = [
        FilingDoor,
        SilentDoor,
        FilingField,
        SilentField,
        DoorController,
        DoorResolver,
        DoorTool,
        DoorGateway,
    ],
)]
struct EveryEdgeModule;

/// The `warn` and `error` lines the framework and the app filed since `seen` of
/// them were — rmcp's own `response error`, which the library files beside any
/// JSON-RPC error it answers, is the library's.
fn filed_since(logs: &LogCapture, seen: usize) -> Vec<CapturedEvent> {
    logs.events()
        .into_iter()
        .filter(|event| event.level == "warn" || event.level == "error")
        .filter(|event| !event.target.starts_with("rmcp"))
        .skip(seen)
        .collect()
}

/// How many `warn` and `error` lines were filed so far.
fn filed(logs: &LogCapture) -> usize {
    filed_since(logs, 0).len()
}

/// The one line `edge`'s denial filed: its decider's when the guard files its
/// own, else the edge's floor.
fn assert_one_line(logs: &LogCapture, seen: usize, edge: &str, guard_files: bool) {
    let lines = filed_since(logs, seen);
    let said: Vec<(&str, &str)> = lines
        .iter()
        .map(|line| (line.target.as_str(), line.message.as_str()))
        .collect();
    assert_eq!(lines.len(), 1, "{edge}: one denial, one line — {said:?}");
    match guard_files {
        true => assert_eq!(
            lines[0].message, FILED,
            "{edge}: the guard's own — {said:?}"
        ),
        false => {
            assert_eq!(lines[0].target, "nest_rs::layers", "{edge}: {said:?}");
            assert!(
                lines[0].message.starts_with("guard denied the "),
                "{edge}: the edge's floor — {said:?}",
            );
        }
    }
}

async fn booted(pool: nest_rs_guards::GuardSpec) -> TestApp {
    TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([pool])
        .build()
        .await
        .expect("every edge boots")
}

/// Every edge's request, refused by the pool's guard.
async fn requests_refused(pool: nest_rs_guards::GuardSpec, guard_files: bool) {
    let logs = LogCapture::install();
    let app = booted(pool).await;

    let seen = filed(&logs);
    app.http()
        .get("/door")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "a route", guard_files);

    let seen = filed(&logs);
    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "/graphql", guard_files);

    let seen = filed(&logs);
    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "/mcp", guard_files);

    let seen = filed(&logs);
    app.http()
        .get("/ws/door")
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "a gateway's upgrade", guard_files);
}

#[tokio::test]
async fn a_request_a_filing_guard_refuses_files_the_guards_line_alone() {
    requests_refused(guard::<FilingDoor>(), true).await;
}

#[tokio::test]
async fn a_request_a_silent_guard_refuses_files_the_edges_line() {
    requests_refused(guard::<SilentDoor>(), false).await;
}

/// Every in-band edge's unit, refused by the pool's guard.
async fn units_refused(pool: nest_rs_guards::GuardSpec, guard_files: bool) {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([pool])
        .build_ws()
        .await
        .expect("every edge serves");

    let seen = filed(&logs);
    let mut socket = app.socket("/ws/door").connect().await;
    socket.send("ping", serde_json::json!({})).await;
    let refused = socket.next_envelope().await;
    assert!(
        refused["data"]["error"].is_string(),
        "the message is refused: {refused}"
    );
    assert_one_line(&logs, seen, "a WS message", guard_files);
    drop(socket);
    app.close().await;

    let app = booted(match guard_files {
        true => guard::<FilingField>(),
        false => guard::<SilentField>(),
    })
    .await;

    let seen = filed(&logs);
    let graphql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    graphql.assert_status(StatusCode::OK);
    assert_one_line(&logs, seen, "a GraphQL operation", guard_files);

    let seen = filed(&logs);
    let mcp = call_tool_as(app.http(), "/mcp", "ping", None, &[], serde_json::json!({})).await;
    assert!(mcp.contains("closed"), "the tool call is refused: {mcp}");
    assert_one_line(&logs, seen, "an MCP operation", guard_files);
}

#[tokio::test]
async fn a_unit_a_filing_guard_refuses_files_the_guards_line_alone() {
    units_refused(guard::<FilingField>(), true).await;
}

#[tokio::test]
async fn a_unit_a_silent_guard_refuses_files_the_edges_line() {
    units_refused(guard::<SilentField>(), false).await;
}
