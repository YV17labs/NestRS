//! Users WebSocket adapter — `UsersGateway` exposes `users.list` over the
//! WS upgrade endpoint. Importing [`UsersWsModule`] mounts it on the HTTP
//! transport (the gateway self-mounts; no separate WS transport).

mod gateway;
mod module;

pub use gateway::UsersGateway;
pub use module::UsersWsModule;
