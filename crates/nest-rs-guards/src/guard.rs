//! The unified [`Guard`] trait — extends [`Layer`] so guards plug into the
//! Layer System (dedup-by-`TypeId`, declaration-order chain).

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::poem::Request as HttpRequest;
#[cfg(feature = "mcp")]
use nest_rs_mcp::McpOperationContext;
#[cfg(feature = "ws")]
use nest_rs_ws::{WsClient, WsMessageCheck};
#[cfg(feature = "ws")]
use serde_json::Value;

use crate::denial::Denial;

#[cfg(feature = "graphql")]
use nest_rs_graphql::async_graphql::Context as GraphqlContext;

/// Where in a guard chain this guard belongs — **declared**, never inferred
/// from type names. Boot-time chain validation reads it to refuse a chain
/// whose authorization guard is listed before the authentication guard it
/// depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardPhase {
    /// Establishes *who* the caller is (attaches a principal).
    Authentication,
    /// Decides *what* the caller may do (consumes a principal).
    Authorization,
    /// Neither — throttling, feature gates, custom checks.
    Other,
}

/// A principal type a guard produces onto — or expects from — the request,
/// described by `TypeId` plus a human-readable name for boot errors.
#[derive(Clone, Copy, Debug)]
pub struct PrincipalClaim {
    /// The principal's `TypeId` (the request-extension key).
    pub type_id: TypeId,
    /// The principal's type name, for boot diagnostics.
    pub type_name: &'static str,
}

impl PrincipalClaim {
    /// Describe the principal type `T`.
    pub fn of<T: 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
        }
    }
}

/// A transport-spanning guard.
///
/// One impl, three transports. Override only the `check_*` method(s) where
/// this guard has work to do; the rest inherit `Ok(())` defaults — a
/// no-op means "doesn't apply to this transport," not "skip security."
///
/// `Guard` extends [`Layer`] (priority + name + dedup-by-TypeId). The
/// `#[public]` marker is NOT a framework skip: it attaches the
/// [`Public`](nest_rs_core::Public) data to the request and each guard
/// decides whether to honor it. An `AbilityGuard` may want to apply
/// visitor rules on public routes; an `AuthnGuard` may want to skip
/// rejection when no token is present. Both are policy decisions the
/// guard owns, not the framework.
///
/// See the crate-level docs for copy-paste templates.
#[async_trait]
pub trait Guard: Layer {
    /// HTTP request entry. Default = no-op (this guard doesn't apply to HTTP).
    async fn check_http(&self, _req: &mut HttpRequest) -> Result<(), Denial> {
        Ok(())
    }

    /// GraphQL resolver entry. Default = no-op. Available with the `graphql`
    /// feature on this crate.
    #[cfg(feature = "graphql")]
    async fn check_graphql(&self, _ctx: &GraphqlContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    /// WebSocket per-message entry. Default = no-op. Available with the `ws`
    /// feature on this crate.
    #[cfg(feature = "ws")]
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &Value,
    ) -> Result<(), Denial> {
        Ok(())
    }

    /// MCP per-operation entry. Default = no-op. Available with the `mcp`
    /// feature on this crate.
    ///
    /// Runs inside rmcp's spawned dispatch, *after* the endpoint's
    /// [`McpOperationGuard`](nest_rs_mcp::McpOperationGuard) has authenticated
    /// the request and installed the caller's `Ability` — so a capability-only
    /// guard reads the ability with `nest_rs_authz::current_ability` here, the
    /// same way `check_http` reads it off the request.
    ///
    /// The context deliberately carries no arguments: rejecting a *payload* is
    /// a pipe's job (`Valid<T>` / `Piped<P, T>` run on the wire value before the
    /// body), and a guard that reads arguments is one step from deciding access
    /// from them.
    #[cfg(feature = "mcp")]
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    /// The chain phase this guard declares. Boot-time validation refuses a
    /// chain listing an [`Authentication`](GuardPhase::Authentication) guard
    /// after an [`Authorization`](GuardPhase::Authorization) one.
    fn phase(&self) -> GuardPhase {
        GuardPhase::Other
    }

    /// The principal type this guard attaches to the request on success
    /// (an authn guard's claims), if any. Read by boot-time chain validation.
    fn produced_principal(&self) -> Option<PrincipalClaim> {
        None
    }

    /// The principal type this guard expects an earlier guard to have
    /// attached (an authz guard's actor), if any. Boot-time chain validation
    /// fails boot when an earlier guard produces a *different* principal type
    /// — the mismatch that would otherwise 500 on every request.
    fn expected_principal(&self) -> Option<PrincipalClaim> {
        None
    }
}

/// A guard that checks GraphQL operations — declared by overriding
/// [`Guard::check_graphql`] and then writing `impl GraphqlGuard for X {}`.
///
/// Every `check_*` defaults to `Ok(())`, which is right ("does not apply to this
/// transport") and was also the answer to a question nobody asked:
/// `#[use_guards(ThrottlerGuard)]` beside a `#[query]` compiled, read as a
/// protection, and throttled nothing — that guard implements only `check_http`, so
/// the per-operation chain called the default and passed. `#[resolver]` and
/// `#[operations]` now emit a bound against this marker for every guard declared
/// at their site, so the mistake is a compile error at the `#[use_guards]` line.
///
/// Declaring the marker without overriding the method is the one mistake left. It
/// is a deliberate line, written next to the method it should have been.
#[cfg(feature = "graphql")]
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not check GraphQL operations",
    label = "this guard has no `check_graphql`",
    note = "a guard bound on a resolver runs `Guard::check_graphql`, whose default is \
            `Ok(())` — binding one that does not override it passes every operation silently",
    note = "override `check_graphql`, then declare `impl GraphqlGuard for {Self} {{}}`; or bind \
            this guard on an HTTP `#[controller]` / `#[routes]` instead"
)]
pub trait GraphqlGuard: Guard {}

/// A guard that checks WebSocket messages — declared by overriding
/// [`Guard::check_ws_message`] and then writing `impl WsGuard for X {}`.
///
/// Required by `#[messages]` of every guard in a `#[use_guards]` beside a
/// `#[subscribe_message]`. **Not** of a `#[gateway]`-struct guard: those run on the
/// upgrade, which is an HTTP `GET`, so they are HTTP guards — and that distinction
/// is exactly what this bound makes visible instead of leaving to a reader.
#[cfg(feature = "ws")]
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not check WebSocket messages",
    label = "this guard has no `check_ws_message`",
    note = "a guard bound beside a `#[subscribe_message]` runs `Guard::check_ws_message`, \
            whose default is `Ok(())` — binding one that does not override it passes every \
            message silently",
    note = "override `check_ws_message`, then declare `impl WsGuard for {Self} {{}}`; or move it \
            to the `#[gateway]` struct, where guards run on the HTTP upgrade instead"
)]
pub trait WsGuard: Guard {}

/// A guard that checks MCP operations — declared by overriding [`Guard::check_mcp`]
/// and then writing `impl McpGuard for X {}`.
///
/// Required by `#[mcp]` of every host-scope guard and by `#[tools]` of every
/// operation-scope one: both fold into the same per-operation chain, so both run
/// `check_mcp`.
#[cfg(feature = "mcp")]
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not check MCP operations",
    label = "this guard has no `check_mcp`",
    note = "a guard bound on an MCP host or operation runs `Guard::check_mcp`, whose default \
            is `Ok(())` — binding one that does not override it passes every tool call \
            silently",
    note = "override `check_mcp`, then declare `impl McpGuard for {Self} {{}}`; or bind this \
            guard on an HTTP `#[controller]` / `#[routes]` instead"
)]
pub trait McpGuard: Guard {}

// Manual forwards, not `#[async_trait]`: the macro would wrap each inner
// (already boxed) future in a second box, taxing every check made through an
// `Arc<dyn Guard>` without `.as_ref()` — the double-box the dispatch sites
// individually dodge today.
impl<T: Guard + ?Sized> Guard for Arc<T> {
    fn check_http<'s, 'r, 'fut>(
        &'s self,
        req: &'r mut HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>
    where
        's: 'fut,
        'r: 'fut,
        Self: 'fut,
    {
        (**self).check_http(req)
    }

    #[cfg(feature = "graphql")]
    fn check_graphql<'s, 'c, 'g, 'fut>(
        &'s self,
        ctx: &'c GraphqlContext<'g>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>
    where
        's: 'fut,
        'c: 'fut,
        'g: 'fut,
        Self: 'fut,
    {
        (**self).check_graphql(ctx)
    }

    #[cfg(feature = "ws")]
    fn check_ws_message<'s, 'c, 'e, 'd, 'fut>(
        &'s self,
        client: &'c WsClient,
        event: &'e str,
        data: &'d Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>
    where
        's: 'fut,
        'c: 'fut,
        'e: 'fut,
        'd: 'fut,
        Self: 'fut,
    {
        (**self).check_ws_message(client, event, data)
    }

    #[cfg(feature = "mcp")]
    fn check_mcp<'s, 'c, 'o, 'fut>(
        &'s self,
        ctx: &'c McpOperationContext<'o>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>
    where
        's: 'fut,
        'c: 'fut,
        'o: 'fut,
        Self: 'fut,
    {
        (**self).check_mcp(ctx)
    }

    fn phase(&self) -> GuardPhase {
        (**self).phase()
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        (**self).produced_principal()
    }

    fn expected_principal(&self) -> Option<PrincipalClaim> {
        (**self).expected_principal()
    }
}

/// Newtype adapter that lets any [`Guard`] satisfy the
/// [`WsMessageCheck`](nest_rs_ws::WsMessageCheck) interface — the bridge the
/// `#[messages]` macro uses to put guards in the per-event chain table
/// without nest-rs-ws depending on nest-rs-guards.
#[cfg(feature = "ws")]
pub struct GuardAsWsMessageCheck {
    inner: Arc<dyn Guard>,
    type_id: TypeId,
    name: &'static str,
}

#[cfg(feature = "ws")]
impl GuardAsWsMessageCheck {
    /// Adapt an HTTP [`Guard`] into a per-WS-message check, preserving its
    /// `type_id`/`name` so dedup and logging match the HTTP path.
    pub fn new(inner: Arc<dyn Guard>, type_id: TypeId, name: &'static str) -> Self {
        Self {
            inner,
            type_id,
            name,
        }
    }
}

#[cfg(feature = "ws")]
#[async_trait]
impl WsMessageCheck for GuardAsWsMessageCheck {
    async fn check(
        &self,
        client: &WsClient,
        event: &str,
        data: &Value,
    ) -> std::result::Result<(), String> {
        match self.inner.check_ws_message(client, event, data).await {
            Ok(()) => Ok(()),
            Err(denial) => Err(denial.message().to_owned()),
        }
    }

    fn type_key(&self) -> TypeId {
        self.type_id
    }

    fn layer_name(&self) -> &'static str {
        self.name
    }
}
