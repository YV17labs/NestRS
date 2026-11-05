//! `AbilityGuard`'s MCP entry (`src/guard.rs`) through the real
//! decorators: `#[use_guards(AuthzGuard)]` on an `#[mcp]` host, which is the
//! symmetric binding its HTTP, GraphQL and WS siblings already carry.
//!
//! Both directions are asserted: only the pair tells a working check from a
//! deny-all and from a no-op.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Delete, Read};
use nest_rs_core::{Layer, async_trait, injectable, module};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::EndpointGuards;
use nest_rs_mcp::{McpAccess, McpError, McpModule, McpView, mcp, tools};
use nest_rs_testing::{LogCapture, TestApp, mcp::call_tool};

use super::rules_read;
use crate::widget;

const PATH: &str = "/mcp/guard";

/// The app's principal, attached by the authn half exactly as a real one is.
#[derive(Clone)]
struct Actor;

impl nest_rs_core::AmbientValue for Actor {}

/// Stands in for the app's authn guard: attaches the principal `AbilityGuard`
/// builds the caller's rules from, and names it as the authn guard does.
#[injectable]
#[derive(Default)]
struct ActorGuard;

impl Layer for ActorGuard {}

#[async_trait]
impl Guard for ActorGuard {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("actor");
        cx.attach(Actor);
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for ActorGuard {}

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

    fn define_visitor(&self, ability: &mut AbilityBuilder) {
        ability.can(Action::Delete, widget::Entity);
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
    /// Gated, with no mask to arm, so its posture states a caller is needed and
    /// only the ability it reads stands in front of the body.
    #[tool]
    #[authorize(Read, widget::Entity, unmasked)]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }

    /// Admits a caller with no credential, the guard reading who that is.
    #[tool]
    #[public(Delete, widget::Entity, unmasked)]
    async fn open(&self) -> Result<String, McpError> {
        Ok(rules_read())
    }
}

/// The same host's operation under guards that authenticate: they cover it.
#[mcp(path = "/mcp/guard")]
#[use_guards(AuthzGuard)]
#[derive(Clone, Default)]
struct CoveredTool;

#[tools]
impl CoveredTool {
    /// No posture: only the guard chain stands in front of the body.
    #[tool]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

/// The endpoint is opened and nothing runs at its request — as when the app's
/// authz module declaring the endpoint's guards never made it into the module
/// tree.
#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [Rules, AuthzGuard, GuardedTool],
)]
struct UnscopedModule;

/// The wired app: the endpoint's guards authenticate the request and attach the
/// caller's ability, ambient for the operation's duration.
#[module(providers = [
    Rules,
    ActorGuard,
    AuthzGuard,
    EndpointGuards<McpView, (ActorGuard, AuthzGuard)>,
    CoveredTool,
])]
struct ScopedModule;

#[tokio::test]
async fn a_gated_operation_no_ability_reaches_is_refused() {
    // Thread-local: `#[tokio::test]` is a current-thread runtime, so the
    // endpoint's task runs on this thread.
    let logs = LogCapture::install();
    let app = TestApp::for_module::<UnscopedModule>()
        .await
        .expect("a host binding the ability guard boots");

    let body = call_tool(app.http(), PATH, "ping", None).await;
    assert!(
        !body.contains("pong"),
        "its posture needs a caller and nothing decided what this one may do, so the \
         operation fails closed rather than running unscoped: {body}",
    );
    assert!(
        body.contains("error"),
        "…and it refuses loudly, never as a silent passthrough: {body}",
    );

    // The refusal is an opaque JSON-RPC error; only the log says the ability is missing.
    let event = logs.expect_one("nest_rs::authz", "guard denied the operation");
    assert_eq!(
        event.level, "warn",
        "a denial is a security event: `warn` or above, never `debug`",
    );
    assert_eq!(
        event.field("unit").as_deref(),
        Some("mcp.operation"),
        "the line names the unit that was refused, or an operator filtering denials \
         by edge sees every edge except this one: {:?}",
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
async fn an_operation_the_endpoint_guards_scoped_reaches_the_body() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ScopedModule>()
        .await
        .expect("the wired app boots");

    let body = call_tool(app.http(), PATH, "ping", None).await;
    assert!(
        body.contains("pong"),
        "the endpoint's guards attached the caller's ability, so the guard has nothing to \
         refuse — without this half the check above would also pass a deny-all: \
         {body}",
    );
    logs.expect_none("nest_rs::authz", "authorization denied");
}

#[tokio::test]
async fn a_public_operation_under_its_hosts_ability_guard_reads_the_visitors_rules() {
    let app = TestApp::for_module::<UnscopedModule>()
        .await
        .expect("a host binding the ability guard boots");

    let body = call_tool(app.http(), PATH, "open", None).await;
    assert!(body.contains("visitor may delete"), "{body}");
}

#[mcp(path = "/mcp/guard/opened")]
#[derive(Clone, Default)]
struct OpenedTool;

#[tools]
impl OpenedTool {
    /// Admits a caller with no credential: what its posture says on every edge.
    #[tool]
    #[public(Delete, widget::Entity, unmasked)]
    async fn open(&self) -> Result<String, McpError> {
        Ok(rules_read())
    }

    /// Needs a caller, whom an opened endpoint's request never identifies.
    #[tool]
    #[authorize(Read, widget::Entity, unmasked)]
    async fn gated(&self) -> Result<String, McpError> {
        Ok("reached".to_owned())
    }
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [Rules, ActorGuard, AuthzGuard, OpenedTool],
)]
struct OpenedModule;

/// The canonical pool over an endpoint the app opened: the operation's chain folds
/// the pool, whatever opened the request.
#[module(imports = [OpenedModule], providers = [GlobalGuards<(ActorGuard, AuthzGuard)>])]
struct OpenedUnderThePool;

#[tokio::test]
async fn a_public_operation_of_an_opened_endpoint_serves_under_the_canonical_pool() {
    let app = TestApp::for_module::<OpenedUnderThePool>()
        .await
        .expect("an opened endpoint under the pool boots");

    let open = call_tool(app.http(), "/mcp/guard/opened", "open", None).await;
    assert!(open.contains("visitor may delete"), "{open}");

    let gated = call_tool(app.http(), "/mcp/guard/opened", "gated", None).await;
    assert!(
        !gated.contains("reached") && gated.contains("UNAUTHENTICATED"),
        "a gated operation still needs a caller: {gated}",
    );
}
