//! Authz feature — policy ([`AppAbility`] in `core/`) plus per-transport
//! adapters: HTTP guard ([`http::AppAbilityGuard`]), GraphQL operation
//! bridge ([`graphql::AppGraphqlGuard`]), WebSocket connection context
//! ([`ws::AuthzWsModule`]).

pub mod core;
pub mod graphql;
pub mod http;
pub mod ws;

pub use core::{AppAbility, AuthzCoreModule};
pub use graphql::{AppGraphqlGuard, AuthzGraphqlModule};
pub use http::{AppAbilityGuard, AuthzHttpModule};
pub use ws::AuthzWsModule;
