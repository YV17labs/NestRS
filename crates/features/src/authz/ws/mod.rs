//! Authz WebSocket adapter — three pieces:
//!
//! 1. [`WsDataContext`](nestrs_database::ws::WsDataContext) bound as
//!    `dyn SocketContext` — installs the per-message ambient executor +
//!    ability scope, the WS analog of HTTP's ambient `Authorize` shaper.
//! 2. [`WsAuthGuard`] (a `MessageGuard`) — bound per-message via
//!    `#[use_guards(WsAuthGuard)]` so the **access graph** sees that every
//!    feature's gateway depends on this module (without it, the
//!    `SocketContext` provider is invisible to the import contract).
//! 3. [`AuthzWsModule`] itself — registers both.

mod guard;
mod module;

pub use guard::WsAuthGuard;
pub use module::AuthzWsModule;
