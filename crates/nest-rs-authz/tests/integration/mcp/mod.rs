//! Mirror tests for `src/mcp/` — only compiled when the `mcp` feature is on.

use nest_rs_core::{Layer, Module, async_trait, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool_as;

/// Stand-in for the authentication guard at the request: admits it, naming
/// its caller as the authentication guard does.
#[injectable]
#[derive(Default, Layer)]
pub(crate) struct PassGuard;

#[async_trait]
impl Guard for PassGuard {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authentication
    }

    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("caller");
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for PassGuard {}

/// Boot `M` and call `tool` as `role` — the role rides an `x-role` header each
/// suite's `AbilityInjector` reads, which a bearer cannot carry.
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
