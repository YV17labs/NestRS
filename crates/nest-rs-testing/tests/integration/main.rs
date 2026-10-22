//! Integration tests organized by concern rather than mirroring `src/`: this
//! crate's suite drives the whole framework.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod access_contract;
mod client;
mod config;
mod cors;
mod database;
mod denial_lines;
mod destructured_args;
mod endpoint_guards;
mod env_cascade;
mod exception_filters;
mod fail_secure_boot;
mod graphql;
mod guard_markers;
mod guards;
mod harness_parity;
mod http;
mod interceptors;
mod layer_pool;
mod lifecycle_hooks;
mod mcp;
mod pipes;
mod reflector;
mod request_scope;
mod setups;
mod shutdown;
mod transient_scope;
mod transport_parity;
mod versioning_filters;
mod ws;
mod ws_gateway_guards;

use nest_rs_core::WiringFailedError;
use nest_rs_testing::HeadlessApp;

/// Call `tool` on `path` with no arguments as a `2026-07-28` client does,
/// statelessly: no session opens, so none needs a caller to be bound to.
pub(crate) async fn call_tool_statelessly(
    client: &nest_rs_testing::TestClient,
    path: &str,
    tool: &str,
) -> String {
    let version = nest_rs_mcp::ProtocolVersion::LATEST;
    nest_rs_testing::mcp::post_message_with(
        client,
        path,
        None,
        None,
        &[
            ("mcp-protocol-version", version.as_str()),
            ("mcp-method", "tools/call"),
            ("mcp-name", tool),
        ],
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": version.as_str(),
                    "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-testing", "version": "0" },
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }),
    )
    .await
    .0
    .into_body()
    .into_string()
    .await
    .expect("a response body")
}

/// The wiring a headless boot refused with, and the chain it says.
pub(crate) fn refused_wiring(
    result: anyhow::Result<HeadlessApp>,
    expectation: &str,
) -> (&'static str, String) {
    let Err(err) = result else {
        panic!("{expectation}");
    };
    let chain = format!("{err:#}");
    let wiring = err
        .downcast_ref::<WiringFailedError>()
        .unwrap_or_else(|| panic!("the kernel's wiring step refuses it: {chain}"));
    (wiring.registry, chain)
}
