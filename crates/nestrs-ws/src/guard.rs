//! Per-message guards — the WebSocket counterpart of an HTTP `#[use_guards]`,
//! scoped to a single `#[subscribe_message]` instead of the upgrade request.
//!
//! A connection-level guard ([`#[use_guards]`](crate::gateway) on the gateway
//! struct) runs once, on the HTTP upgrade, and reuses the HTTP [`Guard`] trait
//! because the handshake *is* a `poem::Request`. A per-message guard has no such
//! request — it gates an individual envelope after the socket is open — so it
//! gets its own trait, [`MessageGuard`], whose context is the message: the
//! [`WsClient`] that sent it, the event name, and the raw `data`.
//!
//! Bind it on a handler with `#[use_guards(GuardA, GuardB)]` beside the
//! `#[subscribe_message]` attribute; each is resolved from the container (so a
//! guard is an ordinary `#[injectable]` provider with its own dependencies) and
//! the first listed runs first. A guard returning `Err(reason)` short-circuits:
//! the client receives an error frame under the request's event name and the
//! handler never runs.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::server::WsClient;

/// Decides whether one incoming message may be dispatched. The `@UseGuards`
/// analog on a `#[subscribe_message]` handler — distinct from the HTTP [`Guard`]
/// because a message carries no `poem::Request`.
///
/// Return `Ok(())` to allow the message through, or `Err(reason)` to reject it
/// (the client receives `data: { "error": reason }` under the event name and the
/// handler does not run). Unlike the HTTP guard, a message guard does not
/// *attach* context — the [`WsClient`] is already the per-connection handle a
/// handler reads, and per-message extensions are not part of the envelope
/// protocol.
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
///         if data.is_null() {
///             Err("empty payload".into())
///         } else {
///             Ok(())
///         }
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

/// The per-gateway map of event name → its `#[use_guards]` guards, built once at
/// mount by `#[messages]` (which resolves each guard from the container) and
/// shared across every connection. The connection loop consults it by event
/// name before dispatching — generically, so the [`Gateway`](crate::Gateway)
/// dispatcher itself stays guard-unaware.
#[derive(Default)]
pub struct MessageGuardTable {
    by_event: HashMap<&'static str, Vec<Arc<dyn MessageGuard>>>,
}

impl MessageGuardTable {
    /// An empty table — the common case (no handler declares `#[use_guards]`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the guards an event's handler declared. Called by the
    /// `#[messages]`-generated mount closure, once per guarded event.
    pub fn insert(&mut self, event: &'static str, guards: Vec<Arc<dyn MessageGuard>>) {
        self.by_event.insert(event, guards);
    }

    /// Run every guard registered for `event`, in order, against the message.
    /// Returns the first rejection reason, or `Ok(())` if all pass (including the
    /// common no-guards case, an empty iteration).
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
    use crate::server::WsServer;
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
        let server = Arc::new(WsServer::default());
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
