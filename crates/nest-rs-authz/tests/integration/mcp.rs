//! Mirror test for `src/mcp.rs` — only compiled when the `mcp` feature is on.
//!
//! `McpAbilityBridge` is the MCP twin of `GraphqlAbilityBridge`: it runs the
//! same authn→authz chain (`run_ability_chain`) and installs the caller's
//! ambient `Ability` for the operation through `around`. These pin the two
//! behaviours that used to differ from the GraphQL side — the denial's status
//! survives, and the *guard* (not the data context) is what scopes a tool body.

use std::sync::Arc;

use nest_rs_authz::mcp::McpAbilityBridge;
use nest_rs_authz::{AbilityBuilder, Action, current_ability};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_mcp::{
    CallToolResult, ContentBlock, McpError, McpOperationGuard, ServerHandler, mcp, tool,
    tool_handler, tool_router,
};
use nest_rs_testing::{TestApp, mcp::call_tool};
use poem::http::{StatusCode, header};

/// A throwaway SeaORM entity to act as the authorization `Subject`.
mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "widgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// No-op stand-in for the bridge's authentication slot.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {}

/// Stands in for a throttler sitting in the authn slot: it denies with a `429`,
/// the status the bridge used to flatten to `401`.
#[injectable]
#[derive(Default)]
struct RateLimitedGuard;

impl Layer for RateLimitedGuard {}

#[async_trait]
impl Guard for RateLimitedGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::rate_limited(30, "slow down"))
    }
}

/// Stands in for the `AbilityGuard` slot: attaches a Read grant on widgets.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::Id, 1));
        req.extensions_mut()
            .insert(Arc::new(b.build().expect("valid test ability")));
        Ok(())
    }
}

/// Reports whether the ambient `Ability` reached the tool body. No
/// `McpToolContext` is registered anywhere in this suite — the guard's `around`
/// is the only thing that could have installed it.
#[mcp(path = "/mcp")]
#[derive(Clone)]
struct AbilityProbeTool;

#[tool_router]
impl AbilityProbeTool {
    #[tool(description = "Report whether the ambient ability reached this tool body.")]
    async fn probe_ability(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            if current_ability().is_some() {
                "scoped"
            } else {
                "unscoped"
            },
        )]))
    }
}

#[tool_handler]
impl ServerHandler for AbilityProbeTool {}

/// The shape `crates/features` wires: `McpAbilityBridge<AuthnGuard, AuthzGuard>
/// as dyn McpOperationGuard`.
type ScopingGuard = McpAbilityBridge<PassGuard, AbilityInjector>;
type ThrottledGuard = McpAbilityBridge<RateLimitedGuard, AbilityInjector>;

#[module(
    providers = [
        PassGuard,
        AbilityInjector,
        ScopingGuard as dyn McpOperationGuard,
        AbilityProbeTool,
    ],
)]
struct ScopingMcpModule;

#[module(
    providers = [
        RateLimitedGuard,
        AbilityInjector,
        ThrottledGuard as dyn McpOperationGuard,
        AbilityProbeTool,
    ],
)]
struct ThrottledMcpModule;

// D2: on GraphQL the bridge's `around` installs the ability; MCP now answers
// the same way. Nothing here registers an `McpToolContext`, so before this the
// tool body ran unscoped — fail-closed under `Repo`, but silently unscoped for
// any non-`Repo` read.
#[tokio::test]
async fn the_bridge_scopes_a_tool_body_with_no_data_context_registered() {
    let app = TestApp::for_module::<ScopingMcpModule>()
        .await
        .expect("the tool host boots and self-mounts at /mcp");

    let body = call_tool(app.http(), "/mcp", "probe_ability", None).await;

    assert!(
        body.contains("scoped") && !body.contains("unscoped"),
        "the guard's `around` must install the caller's ability inside rmcp's \
         dispatch, with or without an `McpToolContext`: {body}",
    );
}

// D3: the bridge maps the `Denial` it was given instead of answering a blanket
// `401`, so a `429` raised anywhere in the chain reaches the client intact —
// `Retry-After` included.
#[tokio::test]
async fn a_rate_limited_denial_reaches_the_client_as_429() {
    let app = TestApp::for_module::<ThrottledMcpModule>()
        .await
        .expect("boots");

    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp.0
            .headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.as_bytes()),
        Some(b"30".as_slice()),
        "the denial's own status and headers survive the bridge",
    );
}
