//! GraphQL bindings (feature `graphql`) — the resolver analog of
//! [`crate::http`]: [`GraphqlAbilityBridge`] is the per-operation guard that
//! authenticates and installs the ambient ability; [`authorize`] is the
//! class-level gate; [`ability`] accesses the per-request ability. Importing
//! this module submits the `GraphqlContextSeed` that forwards `Arc<Ability>` into
//! each operation's GraphQL context.
//!
//! Data-coupled bindings live in `nest_rs_seaorm::graphql` (`bind`,
//! `LoaderScope`).
//!
//! ```ignore
//! #[resolver]
//! impl UsersResolver {
//!     #[query]
//!     async fn users(&self, ctx: &Context<'_>) -> Result<Vec<User>> {
//!         authorize::<Read, users::Entity>(ctx)?;
//!         // ...
//!     }
//! }
//! ```

mod authorize;
mod bridge;
mod context;

pub use authorize::authorize;
pub use bridge::GraphqlAbilityBridge;
pub use context::ability;
