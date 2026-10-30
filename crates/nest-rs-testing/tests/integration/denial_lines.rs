//! One denial files one line, on every edge: the edge's `guard denied …`, on
//! the target the refusing guard declares (`Guard::target`, `nest_rs::layers`
//! when it declares none), carrying the reason its denial names — a guard
//! writes no line of its own. A guard writing its decision once in `check`
//! refuses an HTTP request with it too.

use nest_rs_core::{Layer, UnitContext, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::StatusCode;
use nest_rs_http::{HttpView, async_trait, controller, routes};
use nest_rs_mcp::{McpError, McpView, mcp, tools};
use nest_rs_testing::mcp::{call_tool_statelessly, initialize_request, post_message};
use nest_rs_testing::{CapturedEvent, LogCapture, TestApp};
use nest_rs_ws::{WsModule, WsView, gateway, messages};

/// The target the declaring guards file on.
const ACME: &str = "acme::door";

/// Why the declaring guards refuse.
const CLOSED: &str = "door_closed";

/// A guard refusing every unit and every request, in one decision, on its own
/// target and naming why.
#[injectable]
#[derive(Layer)]
struct DeclaredDoor;

#[async_trait]
impl Guard for DeclaredDoor {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed").with_reason(CLOSED))
    }

    fn target(&self) -> &'static str {
        ACME
    }
}

impl GuardFor<HttpView> for DeclaredDoor {}

/// A guard refusing every unit and every request, declaring nothing.
#[injectable]
#[derive(Layer)]
struct SilentDoor;

#[async_trait]
impl Guard for SilentDoor {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed"))
    }
}

impl GuardFor<HttpView> for SilentDoor {}

/// A guard admitting every request and refusing every unit it carries, on its
/// own target and naming why: the request differs from the unit, so it writes
/// both entries.
#[injectable]
#[derive(Layer)]
struct DeclaredField;

#[async_trait]
impl Guard for DeclaredField {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed").with_reason(CLOSED))
    }

    fn target(&self) -> &'static str {
        ACME
    }
}

/// A guard admitting every request and refusing every unit it carries,
/// declaring nothing.
#[injectable]
#[derive(Layer)]
struct SilentField;

#[async_trait]
impl Guard for SilentField {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("closed"))
    }
}

impl GuardFor<GraphqlView> for DeclaredField {}
impl GuardFor<McpView> for DeclaredField {}
impl GuardFor<WsView> for DeclaredField {}
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
        DeclaredDoor,
        SilentDoor,
        DeclaredField,
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

/// The one line `edge`'s denial filed: the edge's, on the target the guard
/// `named` declares, carrying the reason its denial names when it names one.
fn assert_one_line(logs: &LogCapture, seen: usize, edge: &str, declared: bool, named: &str) {
    let lines = filed_since(logs, seen);
    let said: Vec<(&str, &str)> = lines
        .iter()
        .map(|line| (line.target.as_str(), line.message.as_str()))
        .collect();
    assert_eq!(lines.len(), 1, "{edge}: one denial, one line — {said:?}");
    let line = &lines[0];
    assert!(
        line.message.starts_with("guard denied the "),
        "{edge}: the edge's line — {said:?}",
    );
    assert_eq!(line.level, "warn", "{edge}: a 403 is no defect");
    assert_eq!(
        line.field("guard").as_deref(),
        Some(named),
        "{edge}: one guard field, one vocabulary — {:?}",
        line.fields,
    );
    match declared {
        true => {
            assert_eq!(line.target, ACME, "{edge}: the guard's target — {said:?}");
            assert_eq!(line.field("reason").as_deref(), Some(CLOSED), "{edge}");
        }
        false => {
            assert_eq!(line.target, "nest_rs::layers", "{edge}: {said:?}");
            assert_eq!(line.field("reason"), None, "{edge}: {:?}", line.fields);
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

/// Every edge's request, refused by `G` in the pool, writing its one decision
/// in `check`.
async fn requests_refused<G: Guard + 'static>(declared: bool) {
    let named = std::any::type_name::<G>();
    let logs = LogCapture::install();
    let app = booted(guard::<G>()).await;

    let seen = filed(&logs);
    app.http()
        .get("/door")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "a route", declared, named);

    let seen = filed(&logs);
    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "/graphql", declared, named);

    let seen = filed(&logs);
    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert_one_line(&logs, seen, "/mcp", declared, named);

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
    assert_one_line(&logs, seen, "a gateway's upgrade", declared, named);
}

#[tokio::test]
async fn a_request_a_declaring_guard_refuses_files_one_line_on_its_target() {
    requests_refused::<DeclaredDoor>(true).await;
}

#[tokio::test]
async fn a_request_a_silent_guard_refuses_files_one_line_on_the_layers_target() {
    requests_refused::<SilentDoor>(false).await;
}

/// Every in-band edge's unit, refused by `G` in the pool.
async fn units_refused<G: Guard + 'static>(declared: bool) {
    let named = std::any::type_name::<G>();
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([guard::<G>()])
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
    assert_one_line(&logs, seen, "a WS message", declared, named);
    drop(socket);
    app.close().await;

    let app = booted(guard::<G>()).await;

    let seen = filed(&logs);
    let graphql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    graphql.assert_status(StatusCode::OK);
    assert_one_line(&logs, seen, "a GraphQL operation", declared, named);

    // No guard names a caller at the request, so no session opens for one: a
    // stateless call reaches the operation.
    let seen = filed(&logs);
    let mcp =
        call_tool_statelessly(app.http(), "/mcp", "ping", None, &[], serde_json::json!({})).await;
    assert!(mcp.contains("closed"), "the tool call is refused: {mcp}");
    assert_one_line(&logs, seen, "an MCP operation", declared, named);
}

#[tokio::test]
async fn a_unit_a_declaring_guard_refuses_files_one_line_on_its_target() {
    units_refused::<DeclaredField>(true).await;
}

#[tokio::test]
async fn a_unit_a_silent_guard_refuses_files_one_line_on_the_layers_target() {
    units_refused::<SilentField>(false).await;
}
