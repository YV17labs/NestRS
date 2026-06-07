//! The unified [`Guard`] trait — extends [`Layer`] so guards plug into the
//! Layer System (dedup-by-`TypeId`, declaration-order chain).

use std::any::TypeId;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_graphql::async_graphql::Context as GraphqlContext;
use nest_rs_http::poem::Request as HttpRequest;
use nest_rs_ws::{WsClient, WsMessageCheck};
use serde_json::Value;

use crate::denial::Denial;

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
/// visitor rules on public routes; an `AuthGuard` may want to skip
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

    /// GraphQL resolver entry. Default = no-op.
    async fn check_graphql(&self, _ctx: &GraphqlContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    /// WebSocket per-message entry. Default = no-op.
    async fn check_ws_message(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &Value,
    ) -> Result<(), Denial> {
        Ok(())
    }
}

#[async_trait]
impl<T: Guard + ?Sized> Guard for Arc<T> {
    async fn check_http(&self, req: &mut HttpRequest) -> Result<(), Denial> {
        (**self).check_http(req).await
    }

    async fn check_graphql(&self, ctx: &GraphqlContext<'_>) -> Result<(), Denial> {
        (**self).check_graphql(ctx).await
    }

    async fn check_ws_message(
        &self,
        client: &WsClient,
        event: &str,
        data: &Value,
    ) -> Result<(), Denial> {
        (**self).check_ws_message(client, event, data).await
    }
}

/// Newtype adapter that lets any [`Guard`] satisfy the
/// [`WsMessageCheck`](nest_rs_ws::WsMessageCheck) interface — the bridge the
/// `#[messages]` macro uses to put guards in the per-event chain table
/// without nest-rs-ws depending on nest-rs-guards.
pub struct GuardAsWsMessageCheck {
    inner: Arc<dyn Guard>,
    type_id: TypeId,
    name: &'static str,
}

impl GuardAsWsMessageCheck {
    pub fn new(inner: Arc<dyn Guard>, type_id: TypeId, name: &'static str) -> Self {
        Self {
            inner,
            type_id,
            name,
        }
    }
}

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
