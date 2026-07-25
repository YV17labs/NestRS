//! The unified [`Guard`] trait — extends [`Layer`] so guards plug into the
//! Layer System (dedup-by-`TypeId`, declaration-order chain).

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::poem::Request as HttpRequest;
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
