//! A pooled guard runs the entries it overrides at every edge, whatever
//! `GuardFor` it declares, which only the decorators bound: one overriding
//! only `check` gates every GraphQL, WS and MCP unit and no route; one
//! overriding only `check_http` gates every route, the WS upgrade and the
//! `/graphql` and `/mcp` requests, and no unit they carry. A missing
//! declaration never opens an edge.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, UnitContext, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{HttpView, async_trait, controller, routes};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::mcp::{call_tool, call_tool_as, initialize_request};
use nest_rs_testing::{TestApp, TestAppBuilder};
use nest_rs_ws::{WsModule, gateway, messages};
use poem::http::StatusCode;

/// What every site below answers, so an answer carrying it is a unit the pool
/// let through.
const REACHED: &str = "reached";

/// The header [`ChecksRequestsOnly`] admits a request on.
const PASS: &str = "x-pass";

#[controller(path = "/plain")]
struct PlainController;

#[routes]
impl PlainController {
    #[get("/")]
    #[public]
    async fn index(&self) -> &'static str {
        REACHED
    }
}

#[resolver]
struct PingResolver;

#[operations]
impl PingResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok(REACHED.to_owned())
    }

    #[query]
    #[public]
    async fn pong(&self) -> GqlResult<String> {
        Ok(REACHED.to_owned())
    }
}

#[mcp(path = "/mcp/markers")]
#[derive(Clone, Default)]
struct PingTool;

#[tools]
impl PingTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok(REACHED.to_owned())
    }
}

#[gateway(path = "/ws/markers")]
struct PingGateway;

#[messages]
impl PingGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        REACHED
    }
}

/// Refuses every unit, and declares no edge.
#[injectable]
#[derive(Default)]
struct ChecksUnitsOnly;

impl Layer for ChecksUnitsOnly {}

#[async_trait]
impl Guard for ChecksUnitsOnly {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("refused by a unit check"))
    }
}

static REQUESTS_CHECKED: AtomicUsize = AtomicUsize::new(0);

/// Admits a request carrying [`PASS`], counting each request it checks, and
/// declares no edge.
#[injectable]
#[derive(Default)]
struct ChecksRequestsOnly;

impl Layer for ChecksRequestsOnly {}

#[async_trait]
impl Guard for ChecksRequestsOnly {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        REQUESTS_CHECKED.fetch_add(1, Ordering::SeqCst);
        if HttpView::of(cx)?.request().header(PASS).is_some() {
            return Ok(());
        }
        Err(Denial::forbidden("refused by a request check"))
    }
}

#[module(
    imports = [GraphqlModule::for_root(None), WsModule],
    providers = [
        ChecksUnitsOnly,
        ChecksRequestsOnly,
        PlainController,
        PingResolver,
        PingTool,
        PingGateway,
    ],
)]
struct EveryEdgeModule;

fn pooling<G: Guard + 'static>() -> TestAppBuilder {
    TestApp::builder()
        .module::<EveryEdgeModule>()
        .use_guards_global([guard::<G>()])
}

/// `{ ping pong }` on `/graphql`, with `headers`.
async fn ping_and_pong(
    app: &TestApp,
    headers: &[(&'static str, &'static str)],
) -> (StatusCode, String) {
    let mut request = app.http().post("/graphql");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request
        .body_json(&serde_json::json!({ "query": "{ ping pong }" }))
        .send()
        .await;
    let status = response.0.status();
    let body = response.0.into_body().into_string().await.expect("a body");
    (status, body)
}

#[tokio::test]
async fn a_pooled_unit_check_gates_every_in_band_unit_and_no_route() {
    let app = pooling::<ChecksUnitsOnly>()
        .build()
        .await
        .expect("a pooled guard declaring no edge boots");

    let route = app.http().get("/plain").send().await;
    route.assert_status_is_ok();
    route.assert_text(REACHED).await;

    let (status, body) = ping_and_pong(&app, &[]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the request itself is admitted: {body}"
    );
    assert!(
        !body.contains(REACHED) && body.contains("refused by a unit check"),
        "every field is refused by the guard's own check: {body}",
    );

    let tool = call_tool(app.http(), "/mcp/markers", "ping", None).await;
    assert!(
        !tool.contains(REACHED) && tool.contains("refused by a unit check"),
        "the operation is refused by the guard's own check: {tool}",
    );
}

#[tokio::test]
async fn a_pooled_unit_check_gates_every_ws_message() {
    let app = pooling::<ChecksUnitsOnly>()
        .build_ws()
        .await
        .expect("a pooled guard declaring no edge boots on a real port");

    let mut socket = app.socket("/ws/markers").connect().await;
    socket.send("ping", serde_json::Value::Null).await;
    let reply = socket.next_envelope().await;
    assert_eq!(
        reply["data"]["error"], "refused by a unit check",
        "the upgrade is admitted and the message refused: {reply}",
    );

    app.shutdown().await.expect("the transport stops cleanly");
}

#[tokio::test]
async fn a_pooled_request_check_gates_every_request_and_no_unit_it_carries() {
    let app = pooling::<ChecksRequestsOnly>()
        .build()
        .await
        .expect("a pooled guard declaring no edge boots");

    app.http()
        .get("/plain")
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let (status, body) = ping_and_pong(&app, &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    app.http()
        .post("/mcp/markers")
        .body_json(&initialize_request())
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    REQUESTS_CHECKED.store(0, Ordering::SeqCst);
    let (status, body) = ping_and_pong(&app, &[(PASS, "1")]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(r#""ping":"reached""#) && body.contains(r#""pong":"reached""#),
        "{body}"
    );
    assert_eq!(
        REQUESTS_CHECKED.load(Ordering::SeqCst),
        1,
        "the request is checked once, and neither of its fields",
    );

    let tool = call_tool_as(
        app.http(),
        "/mcp/markers",
        "ping",
        None,
        &[(PASS, "1")],
        serde_json::json!({}),
    )
    .await;
    assert!(tool.contains(REACHED), "{tool}");
}

#[tokio::test]
async fn a_pooled_request_check_gates_the_ws_upgrade_and_no_message() {
    let app = pooling::<ChecksRequestsOnly>()
        .build_ws()
        .await
        .expect("a pooled guard declaring no edge boots on a real port");

    let refused = app.socket("/ws/markers").try_connect().await;
    assert!(
        refused.is_err(),
        "the upgrade without the header is refused"
    );

    REQUESTS_CHECKED.store(0, Ordering::SeqCst);
    let mut socket = app.socket("/ws/markers").header(PASS, "1").connect().await;
    for _ in 0..2 {
        socket.send("ping", serde_json::Value::Null).await;
        assert_eq!(socket.next_envelope().await["data"], REACHED);
    }
    assert_eq!(
        REQUESTS_CHECKED.load(Ordering::SeqCst),
        1,
        "the upgrade is checked once, and none of its messages",
    );

    app.shutdown().await.expect("the transport stops cleanly");
}
