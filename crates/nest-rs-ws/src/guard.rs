//! Per-event layer table — a frozen, mount-time view of the Layer System
//! chain for each `#[subscribe_message]` event. Distinct from the per-route
//! HTTP path because a WS message carries no `poem::Request` — the same
//! `Guard::check_ws_message` runs, but the chain itself is composed and
//! deduped at gateway mount instead of per-request.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use crate::server::WsClient;

/// Object-safe view of [`nest_rs_guards::Guard::check_ws_message`] so the
/// table can store any guard without importing the trait directly (avoids a
/// nest-rs-ws → nest-rs-guards dep cycle).
///
/// `nest-rs-guards` provides a [`GuardAsWsMessageCheck`](../../nest_rs_guards/struct.GuardAsWsMessageCheck.html)
/// wrapper that adapts any `Guard` to this trait — the `#[messages]` macro
/// emits the wrapper at gateway mount.
#[async_trait::async_trait]
pub trait WsMessageCheck: Send + Sync + 'static {
    /// Returns the message a denied check sends back to the client.
    async fn check(
        &self,
        client: &WsClient,
        event: &str,
        data: &serde_json::Value,
    ) -> Result<(), String>;

    /// Stable identity for dedup by `TypeId` at table construction.
    fn type_key(&self) -> TypeId;

    /// Display name for diagnostics.
    fn layer_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

#[async_trait::async_trait]
impl<T: WsMessageCheck + ?Sized> WsMessageCheck for Arc<T> {
    async fn check(
        &self,
        client: &WsClient,
        event: &str,
        data: &serde_json::Value,
    ) -> Result<(), String> {
        (**self).check(client, event, data).await
    }

    fn type_key(&self) -> TypeId {
        (**self).type_key()
    }

    fn layer_name(&self) -> &'static str {
        (**self).layer_name()
    }
}

/// Per-gateway event-name → guard chain, built once at mount by
/// `#[messages]` from the global + per-message Layer-System chain. Frozen
/// for the rest of the process: the dispatcher just iterates.
#[derive(Default)]
pub struct EventLayerTable {
    by_event: HashMap<&'static str, Vec<Arc<dyn WsMessageCheck>>>,
}

impl EventLayerTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, event: &'static str, chain: Vec<Arc<dyn WsMessageCheck>>) {
        self.by_event.insert(event, chain);
    }

    /// Run every guard in the chain for `event`, in canonical order.
    /// `Ok(())` when the event has no chain (including the no-guards case).
    pub async fn check(
        &self,
        client: &WsClient,
        event: &str,
        data: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(chain) = self.by_event.get(event) else {
            return Ok(());
        };
        for guard in chain {
            guard.check(client, event, data).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{Global, WsServer};
    use async_trait::async_trait;
    use serde_json::json;

    struct Allow;
    struct Deny;

    #[async_trait]
    impl WsMessageCheck for Allow {
        async fn check(&self, _: &WsClient, _: &str, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }

        fn type_key(&self) -> TypeId {
            TypeId::of::<Self>()
        }
    }

    #[async_trait]
    impl WsMessageCheck for Deny {
        async fn check(&self, _: &WsClient, _: &str, _: &serde_json::Value) -> Result<(), String> {
            Err("nope".into())
        }

        fn type_key(&self) -> TypeId {
            TypeId::of::<Self>()
        }
    }

    fn client() -> WsClient {
        let server = Arc::new(WsServer::<Global>::default());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = server.connect(tx);
        WsClient::new(id, server)
    }

    #[tokio::test]
    async fn an_unguarded_event_passes() {
        let table = EventLayerTable::new();
        assert!(table.check(&client(), "anything", &json!(1)).await.is_ok());
    }

    #[tokio::test]
    async fn the_first_denial_short_circuits() {
        let mut table = EventLayerTable::new();
        table.insert("msg", vec![Arc::new(Allow), Arc::new(Deny)]);
        let denied = table.check(&client(), "msg", &json!(1)).await;
        assert_eq!(denied.unwrap_err(), "nope");
    }
}
