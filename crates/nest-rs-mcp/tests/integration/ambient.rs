//! `src/ambient.rs`: what the `/mcp` request carries — its trace, what its
//! guards attached, what a layer around it made ambient — reaches each
//! operation on the task rmcp runs it on, in session and stateless modes.

use std::sync::Arc;

use nest_rs_core::{
    AmbientValue, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable,
    interceptor, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpView};
use nest_rs_mcp::{McpError, McpView, mcp, tools};
use nest_rs_testing::mcp::{open_session, post_message_with, result};
use nest_rs_testing::{TestApp, TestResponse};
use serde_json::json;

const PATH: &str = "/mcp/ambient";

/// What the request's guard says the caller is.
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// What a layer around the `/mcp` request makes ambient for the rest of it.
struct Tenant(&'static str);

impl AmbientValue for Tenant {}

/// Attaches a [`Caller`] to every request.
#[injectable]
#[derive(Default)]
struct Attaching;

impl Layer for Attaching {}

#[async_trait]
impl Guard for Attaching {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(Caller("ada"));
        Ok(())
    }
}

impl GuardFor<HttpView> for Attaching {}

/// Around every HTTP unit, the `/mcp` request among them: runs the rest of it
/// with a [`Tenant`] ambient.
#[interceptor(stage = unit)]
struct Tenancy;

impl Layer for Tenancy {
    fn serves(&self, unit: nest_rs_core::operation_log::Unit) -> bool {
        unit.edge() == Some(nest_rs_core::Edge::Http)
    }
}

#[async_trait]
impl Interceptor for Tenancy {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        nest_rs_core::with_ambient(Arc::new(Tenant("acme")), next.run(cx)).await
    }
}

#[mcp(path = "/mcp/ambient")]
#[derive(Clone, Default)]
struct WhoamiTools;

#[tools]
impl WhoamiTools {
    /// Say what the operation runs under: its trace, its caller, its tenant.
    #[tool]
    #[public]
    async fn whoami(&self) -> Result<String, McpError> {
        let trace =
            nest_rs_core::current_trace_id().map_or_else(|| "none".to_owned(), |id| id.to_hex());
        let caller = nest_rs_core::ambient::<Caller>().map_or("nobody", |caller| caller.0);
        let tenant = nest_rs_core::ambient::<Tenant>().map_or("no tenant", |tenant| tenant.0);
        Ok(format!("{trace} {caller} {tenant}"))
    }
}

#[module(providers = [
    WhoamiTools,
    Attaching,
    Tenancy,
    EndpointGuards<McpView, (Attaching,)>,
])]
struct AmbientModule;

/// What the operation answered it ran under, beside the trace the request
/// carrying it ran in, as its `traceresponse` reports it.
async fn said(response: TestResponse) -> (String, String) {
    let trace = response
        .0
        .headers()
        .get("traceresponse")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('-').nth(1))
        .unwrap_or_else(|| panic!("the request reports its trace"))
        .to_owned();
    let body = response
        .0
        .into_body()
        .into_string()
        .await
        .expect("a response body");
    let said = result(&body)["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the tool answered: {body}"))
        .to_owned();
    (said, trace)
}

#[tokio::test]
async fn a_session_operation_runs_under_its_requests_trace_caller_and_ambient() {
    let app = TestApp::for_module::<AmbientModule>().await.expect("boots");
    let session = open_session(app.http(), PATH, None).await;

    let response = post_message_with(
        app.http(),
        PATH,
        Some(&session),
        None,
        &[],
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "whoami", "arguments": {} },
        }),
    )
    .await;

    let (said, trace) = said(response).await;
    assert_eq!(said, format!("{trace} ada acme"));
}

#[tokio::test]
async fn a_stateless_operation_runs_under_its_requests_trace_caller_and_ambient() {
    let app = TestApp::for_module::<AmbientModule>().await.expect("boots");
    let version = nest_rs_mcp::ProtocolVersion::LATEST;

    let response = post_message_with(
        app.http(),
        PATH,
        None,
        None,
        &[
            ("mcp-protocol-version", version.as_str()),
            ("mcp-method", "tools/call"),
            ("mcp-name", "whoami"),
        ],
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "whoami",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": version.as_str(),
                    "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-mcp", "version": "0" },
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }),
    )
    .await;

    let (said, trace) = said(response).await;
    assert_eq!(said, format!("{trace} ada acme"));
}
