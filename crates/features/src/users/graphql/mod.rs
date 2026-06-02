//! Users GraphQL adapter — field resolvers on the entity plus root
//! queries / mutations. Importing [`UsersGraphqlModule`] lands the
//! resolver in the schema.

mod module;
mod resolver;

pub use module::UsersGraphqlModule;
pub use resolver::UsersResolver;
