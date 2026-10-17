//! Global pipes on GraphQL operation **variables**: a `GlobalPipe` is handed
//! them as `ArgumentKind::Variables` before execution, on a POST and on the
//! graphql-ws socket alike, and a rejection becomes a GraphQL error.

use nest_rs_core::{Edge, Layer, UnitContext, injectable, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::HttpView;
use nest_rs_pipes::{ArgumentKind, GlobalPipe, PipeError, pipe};
use nest_rs_testing::TestApp;
use nest_rs_testing::ws::{WsApp, WsFrame, WsSocket};
use serde_json::Value;

/// Uppercases the `raw` string variable, rejects the literal `"boom"`, and
/// flattens the variables into a string for `"flatten"` — exercises the
/// transform, the pipe's own error, and a pipe that breaks the variables' shape.
#[injectable]
#[derive(Default)]
struct VarPipe;

impl Layer for VarPipe {}

impl GlobalPipe for VarPipe {
    fn transform(
        &self,
        _cx: &UnitContext<'_>,
        argument: ArgumentKind,
        value: &mut Value,
    ) -> Result<(), PipeError> {
        assert_eq!(
            argument,
            ArgumentKind::Variables,
            "/graphql hands its variables"
        );
        if let Some(raw) = value.get("raw").and_then(Value::as_str) {
            if raw == "boom" {
                return Err(PipeError::new("boom is not allowed"));
            }
            if raw == "flatten" {
                *value = Value::String(FLATTENED.to_owned());
                return Ok(());
            }
            let upper = raw.to_uppercase();
            value["raw"] = Value::String(upper);
        }
        Ok(())
    }
}

/// What the pipe flattens the variables into, which no error may quote.
const FLATTENED: &str = "sk_live_51HsecretTOKEN";

#[resolver]
struct EchoResolver;

#[operations]
impl EchoResolver {
    #[query]
    #[public]
    async fn echo(&self, raw: String) -> String {
        raw
    }
}

/// Appends `!` to the `raw` variable, so a second run shows.
#[injectable]
#[derive(Default)]
struct BangPipe;

impl Layer for BangPipe {}

impl GlobalPipe for BangPipe {
    fn transform(
        &self,
        _cx: &UnitContext<'_>,
        _argument: ArgumentKind,
        value: &mut Value,
    ) -> Result<(), PipeError> {
        if let Some(raw) = value.get("raw").and_then(Value::as_str) {
            let banged = format!("{raw}!");
            value["raw"] = Value::String(banged);
        }
        Ok(())
    }
}

/// What one pipe call was handed: the input, the unit's edge, the site's
/// address, and the `x-tenant` header of the request the view holds.
type Seen = (ArgumentKind, Option<Edge>, &'static str, Option<String>);

static SEEN: std::sync::Mutex<Vec<Seen>> = std::sync::Mutex::new(Vec::new());

/// Records what it is handed, and changes nothing.
#[injectable]
#[derive(Default)]
struct RecordingPipe;

impl Layer for RecordingPipe {}

impl GlobalPipe for RecordingPipe {
    fn transform(
        &self,
        cx: &UnitContext<'_>,
        argument: ArgumentKind,
        _value: &mut Value,
    ) -> Result<(), PipeError> {
        let tenant = cx
            .switch_to::<HttpView>()
            .and_then(|view| view.request().header("x-tenant"))
            .map(str::to_owned);
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((argument, cx.edge(), cx.handler().address(), tenant));
        Ok(())
    }
}

#[module(providers = [EchoResolver, VarPipe, BangPipe, RecordingPipe])]
struct EchoFeatureModule;

#[module(imports = [GraphqlModule::for_root(None), EchoFeatureModule])]
struct AppWithVarPipe;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AppWithVarPipe>()
        .use_pipes_global([pipe::<VarPipe>()])
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn a_global_pipe_transforms_operation_variables_before_execution() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "hi" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let echo = json
        .value()
        .object()
        .get("data")
        .object()
        .get("echo")
        .string();
    assert_eq!(echo, "HI");
}

#[tokio::test]
async fn a_rejecting_variable_pipe_surfaces_a_graphql_error() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "boom" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors
        .iter()
        .next()
        .expect("a variable-pipe rejection yields one error");
    assert_eq!(
        first.object().get("message").string(),
        "boom is not allowed"
    );
}

#[tokio::test]
async fn variables_a_pipe_left_unusable_are_reported_without_their_value() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "flatten" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    let first = errors.iter().next().expect("one error");
    let message = first.object().get("message").string().to_owned();
    assert!(!message.contains(FLATTENED), "{message}");
    assert_eq!(
        message,
        "variable pipe produced an invalid variables object: invalid type: a string, expected a map"
    );
}

#[tokio::test]
async fn a_pipe_listed_twice_runs_once() {
    let app = TestApp::builder()
        .module::<AppWithVarPipe>()
        .use_pipes_global([pipe::<BangPipe>(), pipe::<BangPipe>()])
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "hi" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_json(serde_json::json!({ "data": { "echo": "hi!" } }))
        .await;
}

#[tokio::test]
async fn a_pipe_is_handed_a_posts_variables_as_the_graphql_requests_unit() {
    let app = TestApp::builder()
        .module::<AppWithVarPipe>()
        .use_pipes_global([pipe::<RecordingPipe>()])
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");
    let resp = app
        .http()
        .post("/graphql")
        .header("x-tenant", "acme")
        .body_json(&serde_json::json!({
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": "hi" },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let seen = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        seen,
        [(
            ArgumentKind::Variables,
            Some(Edge::Http),
            "/graphql",
            Some("acme".to_owned())
        )],
        "one call, on the variables, with the endpoint's handler and the request's view",
    );
}

async fn socket_app() -> WsApp {
    TestApp::builder()
        .module::<AppWithVarPipe>()
        .use_pipes_global([pipe::<VarPipe>()])
        .build_ws()
        .await
        .expect("the schema boots on a real port")
}

/// The next text frame, parsed.
async fn next_message(socket: &mut WsSocket) -> Value {
    match socket.next_frame().await {
        Some(WsFrame::Text(text)) => serde_json::from_str(&text).expect("a protocol message"),
        other => panic!("expected a protocol message, got {other:?}"),
    }
}

/// `connection_init`, then `connection_ack`.
async fn connected(app: &WsApp) -> WsSocket {
    let mut socket = app
        .socket("/graphql")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .connect()
        .await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    socket
}

/// The `subscribe` message running the echo query on `raw`, as operation `id`.
fn echo_over_the_socket(id: &str, raw: &str) -> String {
    serde_json::json!({
        "id": id,
        "type": "subscribe",
        "payload": {
            "query": "query($raw: String!) { echo(raw: $raw) }",
            "variables": { "raw": raw },
        },
    })
    .to_string()
}

#[tokio::test]
async fn an_operation_on_the_socket_runs_the_global_pipes_over_its_variables() {
    let app = socket_app().await;
    let mut socket = connected(&app).await;
    socket.send_text(echo_over_the_socket("1", "hi")).await;
    let next = next_message(&mut socket).await;
    assert_eq!(next["type"], "next", "{next}");
    assert_eq!(next["payload"]["data"]["echo"], "HI", "{next}");
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_pipe_refusing_an_operation_on_the_socket_answers_its_error_and_the_socket_goes_on() {
    let app = socket_app().await;
    let mut socket = connected(&app).await;
    socket.send_text(echo_over_the_socket("1", "boom")).await;
    let refused = next_message(&mut socket).await;
    assert_eq!(
        refused,
        serde_json::json!({
            "type": "error",
            "id": "1",
            "payload": [{ "message": "boom is not allowed" }],
        }),
        "the operation is refused before it runs, in the protocol's error",
    );

    socket.send_text(echo_over_the_socket("2", "hi")).await;
    let next = next_message(&mut socket).await;
    assert_eq!(
        (&next["id"], &next["payload"]["data"]["echo"]),
        (&"2".into(), &"HI".into()),
        "{next}"
    );
    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_pipe_is_handed_a_socket_operations_variables_with_the_upgrade_request() {
    let app = TestApp::builder()
        .module::<AppWithVarPipe>()
        .use_pipes_global([pipe::<RecordingPipe>()])
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut socket = app
        .socket("/graphql")
        .header("sec-websocket-protocol", "graphql-transport-ws")
        .header("x-tenant", "acme")
        .connect()
        .await;
    socket.send_text(r#"{"type":"connection_init"}"#).await;
    assert_eq!(next_message(&mut socket).await["type"], "connection_ack");
    socket.send_text(echo_over_the_socket("1", "hi")).await;
    let next = next_message(&mut socket).await;
    assert_eq!(next["payload"]["data"]["echo"], "hi", "{next}");
    let seen = SEEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        seen,
        [(
            ArgumentKind::Variables,
            Some(Edge::Http),
            "/graphql",
            Some("acme".to_owned())
        )],
        "one call, on the variables, with the endpoint's handler and the upgrade request's view",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}
