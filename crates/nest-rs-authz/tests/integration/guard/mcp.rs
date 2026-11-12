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

use super::{assert_refused_at_the_request, rules_read};
use crate::widget;

const PATH: &str = "/mcp/guard";

/// The app's principal, attached by the authn half exactly as a real one is.
#[derive(Clone)]
struct Actor;

impl nest_rs_core::AmbientValue for Actor {}

/// Stands in for the app's authn guard: attaches the principal `AbilityGuard`
/// builds the caller's rules from, and names it as the authn guard does.
#[injectable]
#[derive(Default, Layer)]
struct ActorGuard;

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
    /// Admits a caller with no credential, the guard reading who that is.
    #[tool]
    #[public(Delete, widget::Entity)]
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

#[mcp(path = "/mcp/guard")]
#[use_guards(AuthzGuard)]
#[derive(Clone, Default)]
struct UnscopedGatedTool;

#[tools]
impl UnscopedGatedTool {
    /// Gated, so its posture states a caller is needed and
    /// only the ability it reads stands in front of the body.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [Rules, AuthzGuard, UnscopedGatedTool],
)]
struct UnscopedGatedModule;

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

/// A unit of an opened endpoint whose ability nothing attaches names its site, the
/// code every call would answer and the posture lifting it, its request running no
/// guard a binding there could add.
fn assert_refused_at_the_opened_endpoint(
    boot: nest_rs_core::anyhow::Result<impl Sized>,
    site: &str,
    lifted_by: &str,
) {
    let Err(refused) = boot else {
        panic!("{site}: every call would answer `401`, so the boot refuses it");
    };
    let refused = format!("{refused:#}");
    assert!(refused.contains(site), "{refused}");
    assert!(refused.contains("`401`"), "{refused}");
    assert!(
        refused.contains("its endpoint is opened, so no guard runs at its request"),
        "{refused}"
    );
    assert!(refused.contains(lifted_by), "{refused}");
    assert!(refused.contains("stop opening the endpoint"), "{refused}");
}

/// Its posture needs a caller, and nothing at the opened endpoint's request decides
/// what one may do: every call would fail closed, so the boot refuses it.
#[tokio::test]
async fn a_gated_operation_no_ability_reaches_refuses_the_boot() {
    assert_refused_at_the_opened_endpoint(
        TestApp::for_module::<UnscopedGatedModule>().await,
        "UnscopedGatedTool::ping",
        "declare it `#[public(Action, Subject)]`, which keeps its gate",
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
    #[public(Delete, widget::Entity)]
    async fn open(&self) -> Result<String, McpError> {
        Ok(rules_read())
    }
}

#[mcp(path = "/mcp/guard/opened")]
#[derive(Clone, Default)]
struct OpenedGatedTool;

#[tools]
impl OpenedGatedTool {
    /// Needs a caller, whom an opened endpoint's request never identifies.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn gated(&self) -> Result<String, McpError> {
        Ok("reached".to_owned())
    }
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [
        Rules,
        ActorGuard,
        AuthzGuard,
        GlobalGuards<(ActorGuard, AuthzGuard)>,
        OpenedGatedTool,
    ],
)]
struct OpenedGatedUnderThePool;

/// The pool folds into the operation's chain, and an opened endpoint's request runs
/// none of it: the ability guard there builds one for a `#[public]` operation alone.
#[tokio::test]
async fn a_gated_operation_of_an_opened_endpoint_refuses_the_boot_under_the_pool() {
    assert_refused_at_the_opened_endpoint(
        TestApp::for_module::<OpenedGatedUnderThePool>().await,
        "OpenedGatedTool::gated",
        "declare it `#[public(Action, Subject)]`, which keeps its gate",
    );
}

#[mcp(path = "/mcp/guard/opened")]
#[derive(Clone, Default)]
struct OpenedUndeclaredTool;

#[tools]
impl OpenedUndeclaredTool {
    /// No posture: gated by its guards alone, the pool's ability guard among them.
    #[tool]
    async fn listed(&self) -> Result<String, McpError> {
        Ok("reached".to_owned())
    }
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [
        Rules,
        ActorGuard,
        AuthzGuard,
        GlobalGuards<(ActorGuard, AuthzGuard)>,
        OpenedUndeclaredTool,
    ],
)]
struct OpenedUndeclaredUnderThePool;

/// Stating no posture, it is gated by its guards, and the pool's ability guard reads
/// what an opened endpoint's request never attaches: `#[public]` lifts it, a guard at
/// that request cannot.
#[tokio::test]
async fn an_operation_of_an_opened_endpoint_stating_no_posture_is_told_to_say_public() {
    assert_refused_at_the_opened_endpoint(
        TestApp::for_module::<OpenedUndeclaredUnderThePool>().await,
        "OpenedUndeclaredTool::listed",
        "declare it `#[public]`, or `#[public(Action, Subject)]`",
    );
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
}

#[mcp(path = "/mcp/guard/gated")]
#[use_guards(AuthzGuard)]
#[derive(Clone, Default)]
struct GatedAtTheUnitTool;

#[tools]
impl GatedAtTheUnitTool {
    /// Needs a caller, and reads the ability its request attached.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn gated_at_the_unit(&self) -> Result<String, McpError> {
        Ok("reached".to_owned())
    }
}

#[module(providers = [
    Rules,
    ActorGuard,
    AuthzGuard,
    EndpointGuards<McpView, (ActorGuard,)>,
    GatedAtTheUnitTool,
])]
struct GatedAtTheUnitModule;

/// The guard builds an ability at a `#[public]` operation alone: a gated one reads what
/// `/mcp`'s request attached, which identified the caller and attached no ability, so
/// every call would answer `401`.
#[tokio::test]
async fn a_gated_operation_whose_ability_guard_sits_at_the_unit_alone_refuses_the_boot() {
    assert_refused_at_the_request(
        TestApp::for_module::<GatedAtTheUnitModule>().await,
        "gated_at_the_unit",
    );
}

#[mcp(path = "/mcp/guard/undeclared")]
#[use_guards(AuthzGuard)]
#[derive(Clone, Default)]
struct UndeclaredAtTheUnitTool;

#[tools]
impl UndeclaredAtTheUnitTool {
    /// No posture: the guards of its own stand in front of the body.
    #[tool]
    async fn listed(&self) -> Result<String, McpError> {
        Ok("reached".to_owned())
    }
}

#[module(providers = [
    Rules,
    ActorGuard,
    AuthzGuard,
    EndpointGuards<McpView, (ActorGuard,)>,
    UndeclaredAtTheUnitTool,
])]
struct UndeclaredAtTheUnitModule;

/// An operation stating no posture is gated by its guards, so the ability guard there
/// reads what `/mcp`'s request attached, as on an `#[authorize]` one.
#[tokio::test]
async fn an_operation_stating_no_posture_whose_ability_guard_sits_at_the_unit_alone_refuses_the_boot()
 {
    assert_refused_at_the_request(
        TestApp::for_module::<UndeclaredAtTheUnitModule>().await,
        "UndeclaredAtTheUnitTool::listed",
    );
}
