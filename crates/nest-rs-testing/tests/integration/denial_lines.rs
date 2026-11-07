//! One denial files one line, on every edge: the edge's `guard denied …`, on
//! the target the refusing guard declares (`Guard::target`, `nest_rs::layers`
//! when it declares none), carrying the reason its denial names — a guard
//! writes no line of its own. A guard writing its decision once in `check`
//! refuses an HTTP request with it too. The line names the guard as an app
//! writes it and the unit by the coordinates its `mounted` line gives it, so
//! the two join.

use nest_rs_core::__private::short_type_name;
use nest_rs_core::operation_log::OTHER;
use nest_rs_core::{Layer, Module, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::{
    ApiVersioning, DEFAULT_VERSION_HEADER, HttpConfig, HttpModule, HttpView, Method, StatusCode,
    controller, routes,
};
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
    imports = [GraphqlModule, WsModule],
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

/// The coordinates a line names a unit by, beside its host and method.
const COORDINATES: [&str; 8] = [
    "verb", "path", "kind", "field", "event", "tool", "prompt", "resource",
];

/// `line`'s coordinates, each one absent unless `expected` names it, and each
/// one it names as the unit's `mounted` line says it, where one mounts it.
fn assert_joins_its_mount(
    logs: &LogCapture,
    edge: &str,
    line: &CapturedEvent,
    host: &str,
    method: Option<&str>,
    expected: &[(&str, &str)],
) {
    assert_eq!(line.field("host").as_deref(), Some(host), "{edge}");
    assert_eq!(line.field("method").as_deref(), method, "{edge}");
    assert_eq!(line.field("address"), None, "{edge}: {:?}", line.fields);
    for coordinate in COORDINATES {
        let named = expected
            .iter()
            .find(|(name, _)| *name == coordinate)
            .map(|(_, value)| (*value).to_owned());
        assert_eq!(
            line.field(coordinate),
            named,
            "{edge}: {coordinate} in {:?}",
            line.fields,
        );
    }
    let Some(method) = method else {
        return;
    };
    let mounted = logs
        .find(nest_rs_http::target::ROUTES, "mounted")
        .into_iter()
        .find(|mounted| {
            mounted.field("host").as_deref() == Some(host)
                && mounted.field("method").as_deref() == Some(method)
        })
        .unwrap_or_else(|| panic!("{edge}: {host}::{method} mounted"));
    for (coordinate, value) in expected {
        assert_eq!(
            mounted.field(coordinate).as_deref(),
            Some(*value),
            "{edge}: the mount's {coordinate} in {:?}",
            mounted.fields,
        );
    }
}

/// The one line `edge`'s denial filed: the edge's, on the target the guard
/// `named` declares, carrying the reason its denial names when it names one.
fn assert_one_line(
    logs: &LogCapture,
    seen: usize,
    edge: &str,
    declared: bool,
    named: &str,
) -> CapturedEvent {
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
    lines.into_iter().next().expect("one line")
}

#[module(imports = [EveryEdgeModule], providers = [GlobalGuards<(DeclaredDoor,)>])]
struct DeclaredDoorPoolModule;

#[module(imports = [EveryEdgeModule], providers = [GlobalGuards<(SilentDoor,)>])]
struct SilentDoorPoolModule;

#[module(imports = [EveryEdgeModule], providers = [GlobalGuards<(DeclaredField,)>])]
struct DeclaredFieldPoolModule;

#[module(imports = [EveryEdgeModule], providers = [GlobalGuards<(SilentField,)>])]
struct SilentFieldPoolModule;

/// `M`, every edge behind the pool it declares.
async fn booted<M: Module + 'static>() -> TestApp {
    TestApp::builder()
        .module::<M>()
        .build()
        .await
        .expect("every edge boots")
}

/// Every edge's request, refused by `G`, the pool `M` declares, writing its
/// one decision in `check`.
async fn requests_refused<M: Module + 'static, G: Guard + 'static>(declared: bool) {
    let named = short_type_name::<G>();
    let logs = LogCapture::install();
    let app = booted::<M>().await;

    let seen = filed(&logs);
    app.http()
        .get("/door")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let line = assert_one_line(&logs, seen, "a route", declared, &named);
    assert_joins_its_mount(
        &logs,
        "a route",
        &line,
        "DoorController",
        Some("open"),
        &[("verb", "GET"), ("path", "/door")],
    );

    let seen = filed(&logs);
    app.http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let line = assert_one_line(&logs, seen, "/graphql", declared, &named);
    assert_joins_its_mount(
        &logs,
        "/graphql",
        &line,
        "GraphqlModule",
        None,
        &[("verb", "POST"), ("path", "/graphql")],
    );

    let seen = filed(&logs);
    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let line = assert_one_line(&logs, seen, "/mcp", declared, &named);
    assert_joins_its_mount(
        &logs,
        "/mcp",
        &line,
        "McpModule",
        None,
        &[("verb", "POST"), ("path", "/mcp")],
    );

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
    let line = assert_one_line(&logs, seen, "a gateway's upgrade", declared, &named);
    assert_joins_its_mount(
        &logs,
        "a gateway's upgrade",
        &line,
        "DoorGateway",
        None,
        &[("verb", "GET"), ("path", "/ws/door")],
    );
}

#[tokio::test]
async fn a_request_a_declaring_guard_refuses_files_one_line_on_its_target() {
    requests_refused::<DeclaredDoorPoolModule, DeclaredDoor>(true).await;
}

#[tokio::test]
async fn a_request_a_silent_guard_refuses_files_one_line_on_the_layers_target() {
    requests_refused::<SilentDoorPoolModule, SilentDoor>(false).await;
}

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        global_prefix: Some("/api".into()),
        ..HttpConfig::default()
    })],
    providers = [DeclaredDoor, DoorController, GlobalGuards<(DeclaredDoor,)>],
)]
struct PrefixedDoorModule;

/// Under a global prefix the line names the path a client calls, as the route's
/// `mounted` line does.
#[tokio::test]
async fn a_denial_names_the_path_served_under_the_global_prefix() {
    let logs = LogCapture::install();
    let app = booted::<PrefixedDoorModule>().await;

    app.http()
        .get("/api/door")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let line = logs.expect_one(ACME, "guard denied the request");
    assert_joins_its_mount(
        &logs,
        "a route under a prefix",
        &line,
        "DoorController",
        Some("open"),
        &[("verb", "GET"), ("path", "/api/door")],
    );
}

#[controller(path = "/door/versioned", version = "2")]
#[derive(Default)]
struct VersionedDoorController;

#[routes]
impl VersionedDoorController {
    #[get("/")]
    #[public]
    async fn open(&self) -> &'static str {
        "open"
    }
}

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        versioning: ApiVersioning::Header,
        ..HttpConfig::default()
    })],
    providers = [DeclaredDoor, VersionedDoorController, GlobalGuards<(DeclaredDoor,)>],
)]
struct HeaderVersionedDoorModule;

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        versioning: ApiVersioning::MediaType,
        ..HttpConfig::default()
    })],
    providers = [DeclaredDoor, VersionedDoorController, GlobalGuards<(DeclaredDoor,)>],
)]
struct MediaTypeVersionedDoorModule;

#[module(
    imports = [HttpModule::for_root(HttpConfig {
        versioning: ApiVersioning::Uri,
        ..HttpConfig::default()
    })],
    providers = [DeclaredDoor, VersionedDoorController, GlobalGuards<(DeclaredDoor,)>],
)]
struct UriVersionedDoorModule;

/// A denial under `M`'s version strategy, reached at `path` with `stated`: its
/// `path` is where the route mounts, which its `mounted` line and the request's
/// own line say too, with the version a field of its own on the first.
async fn assert_a_versioned_denial_joins_its_mount<M: Module + 'static>(
    logs: &LogCapture,
    path: &str,
    stated: Option<(&str, &str)>,
) {
    let app = booted::<M>().await;
    let request = app.http().get(path);
    let request = match stated {
        Some((header, value)) => request.header(header, value),
        None => request,
    };
    request.send().await.assert_status(StatusCode::FORBIDDEN);

    let line = logs.expect_one(ACME, "guard denied the request");
    assert_eq!(line.field("path").as_deref(), Some("/v2/door/versioned"));
    let mounted = logs.expect_one(nest_rs_http::target::ROUTES, "mounted");
    assert_eq!(mounted.field("version").as_deref(), Some("2"));
    for join in ["host", "method", "verb", "path"] {
        assert_eq!(line.field(join), mounted.field(join), "{join}");
    }
    let request_line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    for join in ["verb", "path"] {
        assert_eq!(request_line.field(join), mounted.field(join), "{join}");
    }
}

/// Under a version a header selects, the denial and the `mounted` line both say
/// where the route mounts.
#[tokio::test]
async fn a_denial_under_a_header_version_joins_its_mount() {
    let logs = LogCapture::install();
    assert_a_versioned_denial_joins_its_mount::<HeaderVersionedDoorModule>(
        &logs,
        "/door/versioned",
        Some((DEFAULT_VERSION_HEADER, "2")),
    )
    .await;
}

/// Under a version the media type states, likewise.
#[tokio::test]
async fn a_denial_under_a_media_type_version_joins_its_mount() {
    let logs = LogCapture::install();
    assert_a_versioned_denial_joins_its_mount::<MediaTypeVersionedDoorModule>(
        &logs,
        "/door/versioned",
        Some(("accept", "application/json; version=2")),
    )
    .await;
}

/// Under a version the path carries, the same two lines say the same, and the
/// version is a field there too.
#[tokio::test]
async fn a_denial_under_a_uri_version_joins_its_mount() {
    let logs = LogCapture::install();
    assert_a_versioned_denial_joins_its_mount::<UriVersionedDoorModule>(
        &logs,
        "/v2/door/versioned",
        None,
    )
    .await;
}

/// A method only the client chose reaches no line: `/mcp` takes every method,
/// and the line says it as the HTTP conventions do.
#[tokio::test]
async fn a_method_only_the_client_chose_is_said_as_the_conventions_say_it() {
    let logs = LogCapture::install();
    let app = booted::<DeclaredDoorPoolModule>().await;

    app.http()
        .request(Method::from_bytes(b"BREW").expect("a token"), "/mcp")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let line = logs.expect_one(ACME, "guard denied the request");
    assert_eq!(line.field("verb").as_deref(), Some(OTHER));
    assert!(!line.quotes("BREW"), "{:?}", line.fields);
}

/// Every in-band edge's unit, refused by `G`, the pool `M` declares.
async fn units_refused<M: Module + 'static, G: Guard + 'static>(declared: bool) {
    let named = short_type_name::<G>();
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<M>()
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
    let line = assert_one_line(&logs, seen, "a WS message", declared, &named);
    assert_joins_its_mount(
        &logs,
        "a WS message",
        &line,
        "DoorGateway",
        Some("ping"),
        &[("path", "/ws/door"), ("event", "ping")],
    );
    let message = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_ws::unit::MESSAGE.name(),
        )
        .into_iter()
        .next()
        .expect("the refused message files its line");
    for (coordinate, value) in [("path", "/ws/door"), ("event", "ping")] {
        assert_eq!(
            message.field(coordinate).as_deref(),
            Some(value),
            "the message's line says the coordinates its denial names: {:?}",
            message.fields,
        );
    }
    drop(socket);
    app.close().await;

    let app = booted::<M>().await;

    let seen = filed(&logs);
    let graphql = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    graphql.assert_status(StatusCode::OK);
    let line = assert_one_line(&logs, seen, "a GraphQL operation", declared, &named);
    assert_joins_its_mount(
        &logs,
        "a GraphQL operation",
        &line,
        "DoorResolver",
        Some("ping"),
        &[("kind", "query"), ("field", "ping")],
    );

    // No guard names a caller at the request, so no session opens for one: a
    // stateless call reaches the operation.
    let seen = filed(&logs);
    let mcp =
        call_tool_statelessly(app.http(), "/mcp", "ping", None, &[], serde_json::json!({})).await;
    assert!(mcp.contains("closed"), "the tool call is refused: {mcp}");
    let line = assert_one_line(&logs, seen, "an MCP operation", declared, &named);
    assert_joins_its_mount(
        &logs,
        "an MCP operation",
        &line,
        "DoorTool",
        Some("ping"),
        &[("path", "/mcp"), ("tool", "ping")],
    );
}

#[tokio::test]
async fn a_unit_a_declaring_guard_refuses_files_one_line_on_its_target() {
    units_refused::<DeclaredFieldPoolModule, DeclaredField>(true).await;
}

#[tokio::test]
async fn a_unit_a_silent_guard_refuses_files_one_line_on_the_layers_target() {
    units_refused::<SilentFieldPoolModule, SilentField>(false).await;
}
