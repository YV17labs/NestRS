//! Orgs GraphQL adapter — field resolvers on the entity plus root
//! queries / mutations.

mod module;
mod resolver;

pub use module::OrgsGraphqlModule;
pub use resolver::OrgsResolver;
