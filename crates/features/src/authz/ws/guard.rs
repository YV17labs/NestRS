use async_trait::async_trait;
use nestrs_core::injectable;
use nestrs_ws::{MessageGuard, WsClient};

/// The WebSocket counterpart of HTTP's `#[use_guards(AuthGuard, AppAbilityGuard)]`
/// and GraphQL's `#[use_guards(GraphqlAuthGuard)]`.
///
/// Bound on a `#[subscribe_message]` so the **access graph** sees a feature's
/// `<Feature>WsModule` depends on `AuthzWsModule` — without it, the per-message
/// `dyn SocketContext` ([`WsDataContext`](nestrs_database::ws::WsDataContext))
/// that installs the ambient executor + ability is invisible to the import
/// contract, and a handler that calls `Repo` would silently run unscoped (or
/// fail) at runtime. The `can_activate` check is a no-op (`Ok(())`) — the
/// connection-level [`AuthGuard`](crate::authn::AuthGuard) +
/// [`AppAbilityGuard`](crate::authz::AppAbilityGuard) on the gateway have
/// already authenticated the caller at HTTP upgrade and the
/// `SocketContext::around` re-installs the captured state per message; this
/// guard's role is the **access-graph marker** that makes the WS authz
/// dependency declarative.
#[injectable]
#[derive(Default)]
pub struct WsAuthGuard;

#[async_trait]
impl MessageGuard for WsAuthGuard {
    async fn can_activate(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), String> {
        Ok(())
    }
}
