//! Mirror tests for `src/mcp/` — only compiled when the `mcp` feature is on.

mod authorize;
mod bridge;
mod mask;

use nest_rs_core::{Layer, Module, injectable};
use nest_rs_guards::Guard;
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool_as;

pub(crate) use crate::widget;

/// No-op stand-in for a bridge's authentication slot. Both suites wire the same
/// one, so it lives here rather than being spelled twice.
#[injectable]
#[derive(Default)]
pub(crate) struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {}

/// Boot `M` and call `tool` as `role` — the role rides an `x-role` header each
/// suite's `AbilityInjector` reads, which is what the bearer slot cannot carry.
pub(crate) async fn call_as<M: Module + 'static>(path: &str, role: &str, tool: &str) -> String {
    let app = TestApp::for_module::<M>()
        .await
        .expect("the host under test boots");
    call_tool_as(
        app.http(),
        path,
        tool,
        None,
        &[("x-role", role)],
        serde_json::json!({}),
    )
    .await
}
