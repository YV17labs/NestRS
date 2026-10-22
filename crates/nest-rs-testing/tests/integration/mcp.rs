//! `mcp`'s drivers against the SDK and the endpoint that answer them.

use nest_rs_core::{Layer, UnitContext, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpView, StatusCode, async_trait};
use nest_rs_mcp::{McpError, McpView, Parameters, ProtocolVersion, input, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{self, call_tool_statelessly, initialize_request, post_message, result};
use serde_json::json;

/// rmcp accepts every older revision forever, so nothing else fails when the
/// driver falls behind.
#[test]
fn the_driver_negotiates_the_sdk_latest_handshake() {
    assert_eq!(
        mcp::PROTOCOL_VERSION,
        ProtocolVersion::LATEST_WITH_INITIALIZE.as_str(),
        "rmcp moved its LATEST_WITH_INITIALIZE: bump `mcp::PROTOCOL_VERSION` to match",
    );
}

/// Admits every request without naming its caller, as a guard reading an API
/// key does.
#[injectable]
#[derive(Default)]
struct AdmitsUnnamed;

impl Layer for AdmitsUnnamed {}

#[async_trait]
impl Guard for AdmitsUnnamed {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for AdmitsUnnamed {}

#[input]
struct EchoArgs {
    said: String,
}

#[mcp(path = "/mcp/echo")]
#[derive(Clone, Default)]
struct EchoTool;

#[tools]
impl EchoTool {
    /// Answer what it was told.
    #[tool]
    #[public]
    async fn echo(&self, Parameters(args): Parameters<EchoArgs>) -> Result<String, McpError> {
        Ok(args.said)
    }
}

#[module(providers = [AdmitsUnnamed, EchoTool, EndpointGuards<McpView, (AdmitsUnnamed,)>])]
struct EchoModule;

/// A gated endpoint opens no session for a principal naming no subject; the
/// stateless driver reaches its tool all the same, the arguments carried.
#[tokio::test]
async fn a_stateless_call_reaches_a_tool_where_no_session_opens() {
    let app = TestApp::for_module::<EchoModule>()
        .await
        .expect("a gated endpoint boots");
    post_message(app.http(), "/mcp/echo", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let body = call_tool_statelessly(
        app.http(),
        "/mcp/echo",
        "echo",
        None,
        &[],
        json!({ "said": "hello" }),
    )
    .await;

    assert_eq!(
        result(&body)["result"]["content"][0]["text"],
        "hello",
        "{body}"
    );
}
