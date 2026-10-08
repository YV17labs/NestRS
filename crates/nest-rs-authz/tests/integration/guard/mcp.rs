//! `AbilityGuard`'s MCP entry (`src/guard.rs`) through the real
//! decorators: `#[use_guards(AuthzGuard)]` on an `#[mcp]` host, which is the
//! symmetric binding its HTTP, GraphQL and WS siblings already carry.
//!
//! Every `Guard::check_*` defaults to `Ok(())`, so both directions are asserted:
//! only the pair tells a working check from a deny-all and from a no-op.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::mcp::McpAbilityBridge;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_mcp::{AllowAllMcpGuard, McpError, McpOperationGuard, mcp, tools};
use nest_rs_testing::{LogCapture, TestApp, mcp::call_tool};

use crate::widget;

const PATH: &str = "/mcp/guard";

/// The app's principal, attached by the authn half exactly as a real one is.
#[derive(Clone)]
struct Actor;

/// Stands in for the app's authn guard: attaches the principal `AbilityGuard`
/// builds the caller's rules from.
#[injectable]
#[derive(Default)]
struct ActorGuard;

impl Layer for ActorGuard {}

#[async_trait]
impl Guard for ActorGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        req.extensions_mut().insert(Actor);
        Ok(())
    }
}

impl HttpGuard for ActorGuard {}

/// One grant, so an admitted operation is admitted against a real ability
/// rather than an empty one.
#[injectable]
#[derive(Default)]
struct Rules;

impl AbilityFactory for Rules {
    type Actor = Actor;

    fn define(&self, _actor: &Self::Actor, ability: &mut AbilityBuilder) {
        ability.can(Action::Read, widget::Entity);
    }
}

/// The alias an app writes, bound here on an MCP host.
type AuthzGuard = AbilityGuard<Rules>;

#[mcp(path = "/mcp/guard")]
#[use_guards(AuthzGuard)]
#[derive(Clone, Default)]
struct GuardedTool;

#[tools]
impl GuardedTool {
    /// `#[public]` on purpose: no `authorize` is emitted, so only the guard chain
    /// stands in front of the body.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

/// The endpoint admits every request and installs nothing — as when
/// `AuthzMcpModule` never made it into the module tree.
#[module(providers = [
    Rules,
    AuthzGuard,
    AllowAllMcpGuard as dyn McpOperationGuard,
    GuardedTool,
])]
struct UnscopedModule;

type Bridge = McpAbilityBridge<ActorGuard, AuthzGuard>;

/// The wired app: the bridge authenticates the request and installs the
/// caller's ability for the operation's duration.
#[module(providers = [
    Rules,
    ActorGuard,
    AuthzGuard,
    Bridge as dyn McpOperationGuard,
    GuardedTool,
])]
struct ScopedModule;

#[tokio::test]
async fn an_operation_with_no_ambient_ability_is_refused() {
    // Thread-local: `#[tokio::test]` is a current-thread runtime, so the
    // endpoint's task runs on this thread.
    let logs = LogCapture::install();
    let app = TestApp::for_module::<UnscopedModule>()
        .await
        .expect("a host binding the ability guard boots");

    let body = call_tool(app.http(), PATH, "ping", None).await;
    assert!(
        !body.contains("pong"),
        "nothing decided what this caller may do, so the operation fails closed \
         rather than running unscoped: {body}",
    );
    assert!(
        body.contains("error"),
        "…and it refuses loudly, never as a silent passthrough: {body}",
    );

    // The refusal is an opaque JSON-RPC error; only the log says the bridge is missing.
    let event = logs.expect_one("nest_rs::authz", "authorization denied");
    assert_eq!(
        event.level, "warn",
        "a denial is a security event: `warn` or above, never `debug`",
    );
    assert_eq!(
        event.field("transport").as_deref(),
        Some("mcp"),
        "the line names the edge that refused, or an operator filtering denials \
         by transport sees every edge except this one: {:?}",
        event.fields,
    );
    assert_eq!(
        event.field("reason").as_deref(),
        Some("no_ambient_ability"),
        "and why, in the value space the other three edges already report in: \
         {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn an_operation_the_bridge_scoped_reaches_the_body() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ScopedModule>()
        .await
        .expect("the wired app boots");

    let body = call_tool(app.http(), PATH, "ping", None).await;
    assert!(
        body.contains("pong"),
        "the bridge installed the caller's ability, so the guard has nothing to \
         refuse — without this half the check above would also pass a deny-all: \
         {body}",
    );
    logs.expect_none("nest_rs::authz", "authorization denied");
}
