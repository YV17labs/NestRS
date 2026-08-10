//! WebSocket bindings (feature `ws`) — the per-message analog of [`crate::mcp`].
//!
//! [`authorize`] is the class-level gate and [`masked_reply_for`] masks a
//! message's reply. A gateway calls neither directly:
//! `#[authorize(Action, Entity)]` beside a `#[subscribe_message]` makes
//! `#[messages]` emit both, exactly as `#[tools]` does on MCP, `#[operations]` on
//! GraphQL and `#[routes]` on HTTP.
//!
//! ```ignore
//! #[messages]
//! impl UsersGateway {
//!     #[subscribe_message("users.list")]
//!     #[authorize(Read, users::Entity)]
//!     async fn list(&self) -> Result<Vec<User>, ServiceError> {
//!         Ok(self.svc.list().await?.iter().map(User::from).collect())
//!     }
//! }
//! ```
//!
//! # Why there is no `WsAbilityBridge`
//!
//! GraphQL and MCP are `EdgePosture::Exempt`, so each needs a bridge to re-run
//! the guard chain *in band* per operation. A WS gateway is `Guarded`: the upgrade
//! is an HTTP `GET`, so the real HTTP guards run on it at the edge and there is no
//! chain to re-run. What the connection *cannot* keep is its task-locals — those
//! unwound when the upgrade returned — so the only thing to re-establish per
//! message is the ambient state, and that is `nest_rs_seaorm::ws::WsDataContext`
//! (`dyn SocketContext`), not a guard.
//!
//! Hence one asymmetry worth stating plainly: on GraphQL and MCP the *guard*
//! installs the ability, on WS the *data context* does. The gate below reads it
//! the same way either way — [`current_ability`](crate::current_ability) — which
//! is what lets one `#[authorize]` mean one thing on all four transports.

mod authorize;
mod mask;

pub use authorize::authorize;
pub use mask::masked_reply_for;
