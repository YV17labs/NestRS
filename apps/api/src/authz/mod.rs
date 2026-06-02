//! Transport wiring for authz on this app (GraphQL operation guard, dataloader scope, WS context).
//! Policy and the HTTP ability guard live in [`domain::authz`].

mod bridge;
mod module;

pub use module::AuthzModule;
