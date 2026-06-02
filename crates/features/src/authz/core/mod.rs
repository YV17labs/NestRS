//! Authz core — the policy ([`AppAbility`]) and the module that registers it.
//! Transport-neutral; consumed by `http/` for the HTTP guard, by `graphql/`
//! for the operation bridge, and by `ws/` for the connection context.

mod ability;
mod module;

pub use ability::AppAbility;
pub use module::AuthzCoreModule;
