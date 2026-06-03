//! Per-message guards — distinct from the connection-level HTTP [`Guard`]
//! because a message carries no `poem::Request`. Bound with
//! `#[use_guards(...)]` beside a `#[subscribe_message]`. Each `Err(reason)`
//! short-circuits to an error frame under the request's event name.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::server::WsClient;

/// Decides whether one incoming message may be dispatched. `Err(reason)` ships
/// to the client as `data: { "error": reason }` under the event name; the
/// handler does not run.
///
/// ```ignore
/// #[nestrs_core::injectable]
/// #[derive(Default)]
/// struct RejectEmpty;
///
/// #[nestrs_ws::async_trait]
/// impl nestrs_ws::MessageGuard for RejectEmpty {
///     async fn can_activate(
///         &self,
///         _client: &nestrs_ws::WsClient,
///         _event: &str,
///         data: &nestrs_ws::serde_json::Value,
///     ) -> Result<(), String> {
///         if data.is_null() { Err("empty payload".into()) } else { Ok(()) }
///     }
/// }
/// ```
#[async_trait]
pub trait MessageGuard: Send + Sync + 'static {
    async fn can_activate(
        &self,
        client: &WsClient,
        event: &str,
        data: &serde_json::Value,
    ) -> Result<(), String>;
}

#[async_trait]
impl<T: MessageGuard + ?Sized> MessageGuard for Arc<T> {
    async fn can_activate(
        &self,
        client: &WsClient,
        event: &str,
        data: &serde_json::Value,
    ) -> Result<(), String> {
        (**self).can_activate(client, event, data).await
    }
}

/// Per-gateway event-name → guards map, built once at mount by `#[messages]`
/// and shared across every connection. Keeping the dispatcher guard-unaware.
#[derive(Default)]
pub struct MessageGuardTable {
    by_event: HashMap<&'static str, Vec<Arc<dyn MessageGuard>>>,
}

impl MessageGuardTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, event: &'static str, guards: Vec<Arc<dyn MessageGuard>>) {
        self.by_event.insert(event, guards);
    }

    /// Run every guard registered for `event`, in order. Returns the first
    /// rejection or `Ok(())` (including the no-guards case).
    pub async fn check(
        &self,
        client: &WsClient,
        event: &str,
        data: &serde_json::Value,
    ) -> Result<(), String> {
        let Some(guards) = self.by_event.get(event) else {
            return Ok(());
        };
        for guard in guards {
            guard.can_activate(client, event, data).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{Global, WsServer};
    use serde_json::json;

    struct Allow;
    struct Deny;

    #[async_trait]
    impl MessageGuard for Allow {
        async fn can_activate(
            &self,
            _: &WsClient,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[async_trait]
    impl MessageGuard for Deny {
        async fn can_activate(
            &self,
            _: &WsClient,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<(), String> {
            Err("nope".into())
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
        let table = MessageGuardTable::new();
        assert!(table.check(&client(), "anything", &json!(1)).await.is_ok());
    }

    #[tokio::test]
    async fn the_first_denial_short_circuits() {
        let mut table = MessageGuardTable::new();
        table.insert("msg", vec![Arc::new(Allow), Arc::new(Deny)]);
        let denied = table.check(&client(), "msg", &json!(1)).await;
        assert_eq!(denied.unwrap_err(), "nope");
    }
}
