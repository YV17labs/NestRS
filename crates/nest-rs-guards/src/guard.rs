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
use nest_rs_graphql::GraphqlOperationContext;

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
/// One impl, four transports. Override only the `check_*` method(s) where
/// this guard has work to do; the rest inherit `Ok(())` defaults — a
/// no-op means "doesn't apply to this transport," not "skip security."
///
/// `#[public]` is not a framework skip: each guard reads the
/// [`Public`](nest_rs_http::Public) marker and decides. See the crate-level docs
/// for templates.
#[async_trait]
pub trait Guard: Layer {
    /// HTTP request entry. Default = no-op (this guard doesn't apply to HTTP).
    ///
    /// A guard bound on a `#[controller]` / `#[routes]` must attest it with
    /// [`HttpGuard`].
    async fn check_http(&self, _req: &mut HttpRequest) -> Result<(), Denial> {
        Ok(())
    }

    /// GraphQL operation entry. Default = no-op. Available with the `graphql`
    /// feature on this crate.
    ///
    /// A [`GraphqlOperationContext`] rather than async-graphql's `Context`: the
    /// `_service` / `_entities` root fields have none, and `operation.context()`
    /// reaches it where it exists.
    #[cfg(feature = "graphql")]
    async fn check_graphql(&self, _operation: &GraphqlOperationContext<'_>) -> Result<(), Denial> {
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
    /// Runs after the endpoint's
    /// [`McpOperationGuard`](nest_rs_mcp::McpOperationGuard) installed the
    /// caller's `Ability`. The context carries no arguments: rejecting a payload
    /// is a pipe's job.
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

/// A guard that checks HTTP requests — declared by overriding
/// [`Guard::check_http`] and then writing `impl HttpGuard for X {}`.
///
/// Required by `#[controller]` / `#[routes]` / `#[crud]` of every guard in a
/// `#[use_guards]`, and by `#[gateway]` of every guard on the gateway struct:
/// those run on the WS upgrade, which is an HTTP `GET`.
///
/// The one marker with no feature behind it: HTTP is the substrate the other
/// three edges mount on.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not check HTTP requests",
    label = "this guard has no `check_http`",
    note = "a guard bound on a `#[controller]` / `#[routes]`, or on a `#[gateway]` struct \
            (whose guards run on the upgrade, an HTTP `GET`), runs `Guard::check_http` — whose \
            default is `Ok(())`, so binding one that does not override it passes every request \
            silently",
    note = "override `check_http`, then declare `impl HttpGuard for {Self} {{}}`; or bind this \
            guard at the edge it does check — a `#[resolver]` (`GraphqlGuard`), beside a \
            `#[subscribe_message]` (`WsGuard`, per message rather than at the upgrade), or an \
            `#[mcp]` host (`McpGuard`)"
)]
pub trait HttpGuard: Guard {}

/// A guard that checks GraphQL operations — declared by overriding
/// [`Guard::check_graphql`] and then writing `impl GraphqlGuard for X {}`.
///
/// `#[resolver]` and `#[operations]` bound every guard declared at their site
/// on it, so a guard that does not check GraphQL is a compile error at the
/// `#[use_guards]` line.
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
/// `#[subscribe_message]`. **Not** of a `#[gateway]`-struct guard: those run on
/// the upgrade, an HTTP `GET`, so they are HTTP guards.
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

// Manual forwards, not `#[async_trait]`: the macro would box each already-boxed
// inner future a second time.
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
        operation: &'c GraphqlOperationContext<'g>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>
    where
        's: 'fut,
        'c: 'fut,
        'g: 'fut,
        Self: 'fut,
    {
        (**self).check_graphql(operation)
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
/// [`WsMessageCheck`] interface — the bridge the
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
