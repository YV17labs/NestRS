//! The documented import, booted, and the header a caller reads back: the
//! interceptor is registered by the import, so only a boot can see it.

use nest_rs_core::__private::InterceptorRegistration;
use nest_rs_core::{
    Discovery, Interceptor, Layer, Next, Outcome, UnitContext, injectable, interceptor, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::StatusCode;
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_server_timing::ServerTimingModule;
use nest_rs_testing::TestApp;

use crate::{BareModule, in_profile, probe_timing, server_timing};

#[tokio::test]
async fn the_documented_import_puts_the_header_on_a_response() {
    let timing = probe_timing::<BareModule>()
        .await
        .expect("the import is what attaches the header");

    // W3C Server-Timing §3: a comma-separated list of metrics, each a name with
    // optional `;dur=` and `;desc=` parameters.
    assert!(
        timing.contains("db;dur="),
        "the sub-step a handler recorded reaches the wire: {timing}",
    );
}

/// Answers each status twice: as the handler's `Err`, and as a response it
/// built itself.
#[controller(path = "/status")]
struct StatusController;

#[routes]
impl StatusController {
    #[get("/err/{code}")]
    #[public]
    async fn err(&self, code: nest_rs_http::Path<u16>) -> nest_rs_http::Result<String> {
        Err(nest_rs_http::HttpError::from_status(status(code.0)))
    }

    #[get("/ok/{code}")]
    #[public]
    async fn ok(&self, code: nest_rs_http::Path<u16>) -> nest_rs_http::Response {
        nest_rs_http::Response::builder()
            .status(status(code.0))
            .finish()
    }
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).expect("a status the test names")
}

#[module(imports = [ServerTimingModule], providers = [StatusController])]
struct StatusModule;

/// The `server-timing` header `/status/{shape}/{code}` answers with, once its
/// status is checked.
async fn status_timing(app: &TestApp, shape: &str, code: u16) -> Option<String> {
    let resp = app
        .http()
        .get(format!("/status/{shape}/{code}"))
        .send()
        .await;
    resp.assert_status(status(code));
    server_timing(&resp)
}

/// W3C Server Timing §4: a server may give its metrics to authenticated callers
/// "and nothing at all to all others" — so no refusal of a credential or of a
/// rate is a timing oracle, whatever the profile.
#[tokio::test]
async fn an_authentication_or_rate_refusal_carries_no_timing() {
    let app = TestApp::for_module::<StatusModule>().await.expect("boots");

    for code in [401, 403, 407, 429] {
        for shape in ["err", "ok"] {
            assert_eq!(
                status_timing(&app, shape, code).await,
                None,
                "a {code} answered as `{shape}` carries no timing",
            );
        }
    }

    for code in [400, 404, 500, 503] {
        for shape in ["err", "ok"] {
            assert!(
                status_timing(&app, shape, code).await.is_some(),
                "any other error still says where its time went ({code}, `{shape}`)",
            );
        }
    }
}

/// The header both gates below admit on; any other request is refused `401`.
const ADMIT: &str = "x-admit";

#[injectable]
#[derive(Default)]
struct AdmitGuard;

impl Layer for AdmitGuard {}

#[async_trait]
impl Guard for AdmitGuard {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        if nest_rs_http::HttpView::of(cx)?
            .request()
            .header(ADMIT)
            .is_some()
        {
            Ok(())
        } else {
            Err(Denial::unauthorized("no"))
        }
    }
}

impl GuardFor<nest_rs_http::HttpView> for AdmitGuard {}

#[controller(path = "/private")]
struct PrivateController;

#[routes]
impl PrivateController {
    #[get("/")]
    async fn index(&self) -> String {
        "secret".into()
    }
}

#[module(imports = [ServerTimingModule], providers = [PrivateController, AdmitGuard])]
struct GuardedModule;

/// The `server-timing` header `/private` answers with, once its status is
/// checked: `200` when the request bears [`ADMIT`], `401` otherwise.
async fn private_timing(app: &TestApp, admitted: bool) -> Option<String> {
    let mut req = app.http().get("/private");
    if admitted {
        req = req.header(ADMIT, "1");
    }
    let resp = req.send().await;
    resp.assert_status(if admitted {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    });
    server_timing(&resp)
}

#[tokio::test]
async fn a_guard_denial_carries_no_timing() {
    let app = TestApp::builder()
        .module::<GuardedModule>()
        .use_guards_global([guard::<AdmitGuard>()])
        .build()
        .await
        .expect("boots");

    assert!(
        private_timing(&app, true).await.is_some(),
        "the request the guard admits is stamped",
    );
    assert_eq!(
        private_timing(&app, false).await,
        None,
        "the guard chain's refusal is no oracle",
    );
}

/// A global interceptor refusing with a failure of its own, which the edge
/// answers inside the unit stage the header reads.
#[injectable]
#[derive(Default)]
struct AdmitInterceptor;

impl Layer for AdmitInterceptor {}

#[async_trait]
impl Interceptor for AdmitInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let admitted = nest_rs_http::HttpView::of(cx)?
            .request()
            .header(ADMIT)
            .is_some();
        if !admitted {
            return Err(Denial::unauthorized("no").into());
        }
        next.run(cx).await
    }
}

#[module(imports = [ServerTimingModule], providers = [PrivateController, AdmitInterceptor])]
struct GatedModule;

#[tokio::test]
async fn a_pool_interceptor_refusal_carries_no_timing() {
    let app = TestApp::builder()
        .module::<GatedModule>()
        .use_interceptors_global([interceptor::<AdmitInterceptor>()])
        .build()
        .await
        .expect("boots");

    assert!(
        private_timing(&app, true).await.is_some(),
        "the request the interceptor admits is stamped",
    );
    assert_eq!(
        private_timing(&app, false).await,
        None,
        "an interceptor's refusal is no oracle",
    );
}

/// How many interceptors the bare import registers, booted in the process's
/// profile.
async fn bare_wraps() -> usize {
    let app = TestApp::for_module::<BareModule>()
        .await
        .expect("the import boots");
    Discovery::new(app.container())
        .meta::<InterceptorRegistration>()
        .len()
}

#[test]
fn a_switched_off_header_mounts_nothing() {
    assert_eq!(
        in_profile("production", &[], bare_wraps),
        0,
        "off, the import registers no interceptor: a request pays nothing for it",
    );
    assert_eq!(
        in_profile("development", &[], bare_wraps),
        1,
        "on, the import registers its one interceptor",
    );
}

/// Admits every request, and refuses every in-band unit — a GraphQL field, an
/// MCP operation — a rate.
#[injectable]
#[derive(Default)]
struct OperationLimit;

impl Layer for OperationLimit {}

#[async_trait]
impl Guard for OperationLimit {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::rate_limited(30, "slow down"))
    }
}

impl GuardFor<nest_rs_http::HttpView> for OperationLimit {}
impl GuardFor<nest_rs_graphql::GraphqlView> for OperationLimit {}
impl GuardFor<nest_rs_mcp::McpView> for OperationLimit {}

#[nest_rs_graphql::resolver]
struct LimitedResolver;

#[nest_rs_graphql::operations]
impl LimitedResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }
}

#[module(
    imports = [ServerTimingModule, nest_rs_graphql::GraphqlModule::for_root(None)],
    providers = [OperationLimit, LimitedResolver],
)]
struct LimitedGraphqlModule;

/// A refusal answered inside a `200` — a GraphQL field its guards rate
/// limited — is no oracle either: the request it ran in goes untimed.
#[tokio::test]
async fn a_graphql_field_its_guards_refused_leaves_its_request_untimed() {
    let app = TestApp::builder()
        .module::<LimitedGraphqlModule>()
        .use_guards_global([guard::<OperationLimit>()])
        .build()
        .await
        .expect("boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let timing = crate::server_timing(&resp);
    let body: serde_json::Value = resp.json().await.value().deserialize();
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "RATE_LIMITED",
        "{body}"
    );
    assert_eq!(timing, None, "the refused field marked its request");
}

#[nest_rs_mcp::mcp(path = "/mcp/limited")]
#[derive(Clone, Default)]
struct LimitedTools;

#[nest_rs_mcp::tools]
impl LimitedTools {
    /// Answers nothing: its guards refuse it first.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".into())
    }
}

#[module(
    imports = [ServerTimingModule, nest_rs_mcp::McpModule::for_root(None)],
    providers = [OperationLimit, LimitedTools],
)]
struct LimitedMcpModule;

/// The same for an MCP operation its guards refused: a JSON-RPC error inside
/// a `200`.
#[tokio::test]
async fn an_mcp_operation_its_guards_refused_leaves_its_request_untimed() {
    use nest_rs_testing::mcp::{open_session, post_message, result};

    let app = TestApp::builder()
        .module::<LimitedMcpModule>()
        .use_guards_global([guard::<OperationLimit>()])
        .build()
        .await
        .expect("boots");

    let session = open_session(app.http(), "/mcp/limited", None).await;
    let resp = post_message(
        app.http(),
        "/mcp/limited",
        Some(&session),
        None,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": "ping", "arguments": {} },
        }),
    )
    .await;
    resp.assert_status_is_ok();
    let timing = crate::server_timing(&resp);
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a JSON-RPC answer");
    assert!(result(&body).get("error").is_some(), "{body}");
    assert_eq!(timing, None, "the refused operation marked its request");
}
