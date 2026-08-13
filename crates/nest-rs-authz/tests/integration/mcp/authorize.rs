//! The class-level gate through the real macro: `#[authorize(Action, Entity)]`
//! beside a `#[tool]` makes `#[mcp]` emit `nest_rs_authz::mcp::authorize`
//! before the body — the host writes no gate call.
//!
//! What is pinned here is the *posture*, and it is the same one GraphQL
//! declares: a caller with the grant proceeds, one without is refused, the
//! anonymous caller is refused for want of a principal whatever the visitor
//! branch granted, and a refusal a wider token would have fixed says so.

use std::sync::Arc;

use nest_rs_authz::mcp::McpAbilityBridge;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_mcp::{McpError, McpOperationGuard, mcp, tools};

use super::{PassGuard, widget};

/// Builds the caller's ability from an `x-role` header. `visitor` is the
/// anonymous branch — a *grant* it holds must still not satisfy the gate.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let role = req
            .headers()
            .get("x-role")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let mut builder = AbilityBuilder::new();
        match role.as_str() {
            "admin" => {
                builder.can(Action::Read, widget::Entity);
            }
            // A scope-aware credential that was delegated nothing: the rule is
            // withheld rather than absent, so the refusal can name what to ask
            // for. An ability with no `with_granted_scopes` at all means "not
            // scope-aware", and scoped rules would apply in full.
            "scoped" => {
                builder = AbilityBuilder::new().with_granted_scopes(Some(Arc::from([])));
                builder
                    .can(Action::Read, widget::Entity)
                    .requires_scope("widgets:read");
            }
            // The anonymous branch, holding a real grant: the gate must still
            // refuse it, or a grant written to serve a `#[public]` operation
            // would quietly satisfy every `#[authorize]` one on the same entity.
            "visitor" => {
                builder.can(Action::Read, widget::Entity);
                req.extensions_mut().insert(Arc::new(
                    builder.build_visitor().expect("valid test ability"),
                ));
                return Ok(());
            }
            _ => {}
        }
        req.extensions_mut()
            .insert(Arc::new(builder.build().expect("valid test ability")));
        Ok(())
    }
}

impl HttpGuard for AbilityInjector {}

#[mcp(path = "/mcp/gate")]
#[derive(Clone, Default)]
struct GateTool;

#[tools]
impl GateTool {
    /// Answer only for a caller the class gate admits.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn read_widgets(&self) -> Result<String, McpError> {
        Ok("widgets".to_owned())
    }

    /// The opt-out, for an operation with no entity behind it.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

type Bridge = McpAbilityBridge<PassGuard, AbilityInjector>;

#[module(providers = [
    PassGuard,
    AbilityInjector,
    Bridge as dyn McpOperationGuard,
    GateTool,
])]
struct GateModule;

async fn call(role: &str, tool: &str) -> String {
    super::call_as::<GateModule>("/mcp/gate", role, tool).await
}

#[tokio::test]
async fn a_granted_caller_reaches_the_body() {
    let body = call("admin", "read_widgets").await;
    assert!(
        body.contains("widgets"),
        "the gate admits a caller holding the class grant: {body}",
    );
}

#[tokio::test]
async fn an_ungranted_caller_is_refused_before_the_body() {
    let body = call("nobody", "read_widgets").await;
    assert!(
        body.contains("forbidden"),
        "the gate refuses a caller with no class grant: {body}",
    );
    assert!(!body.contains("widgets"), "…and the body never ran: {body}",);
}

#[tokio::test]
async fn the_anonymous_caller_is_refused_whatever_the_visitor_branch_granted() {
    let body = call("visitor", "read_widgets").await;
    assert!(
        body.contains("unauthenticated"),
        "authentication is decided first and separately, so a visitor grant \
         written to serve a `#[public]` operation cannot satisfy an \
         `#[authorize]` one: {body}",
    );
}

#[tokio::test]
async fn a_refusal_a_wider_token_would_fix_says_so() {
    let body = call("scoped", "read_widgets").await;
    assert!(
        body.contains("insufficient_scope") && body.contains("widgets:read"),
        "a scope refusal is actionable and a plain `forbidden` is final — a \
         client that cannot tell them apart either gives up too early or \
         retries forever: {body}",
    );
}

#[tokio::test]
async fn a_public_operation_has_no_gate() {
    let body = call("nobody", "ping").await;
    assert!(
        body.contains("pong"),
        "`#[public]` declares the operation has no entity to gate — the endpoint \
         still authenticated the caller: {body}",
    );
}
