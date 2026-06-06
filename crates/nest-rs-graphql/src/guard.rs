//! [`ResolverGuard`] — distinct from HTTP's `Guard` because a resolver has no
//! `poem::Request`, it has an [`async_graphql::Context`]. Authentication and
//! ability-building stay request-level (the `OperationGuard` bridge seeds the
//! principal / `Ability` into the context); a `ResolverGuard` reads that
//! seeded state to gate one operation.
//!
//! ```ignore
//! #[nest_rs_core::injectable]
//! #[derive(Default)]
//! struct AdminOnly;
//!
//! #[nest_rs_graphql::async_trait]
//! impl nest_rs_graphql::ResolverGuard for AdminOnly {
//!     async fn check(&self, ctx: &nest_rs_graphql::async_graphql::Context<'_>)
//!         -> nest_rs_graphql::async_graphql::Result<()> {
//!         match ctx.data_opt::<Principal>() {
//!             Some(p) if p.is_admin() => Ok(()),
//!             _ => Err(nest_rs_graphql::async_graphql::Error::new("forbidden")),
//!         }
//!     }
//! }
//! ```

use async_graphql::{Context, Result};
use async_trait::async_trait;

/// Runs before a resolver operation; `Err(error)` short-circuits with that
/// GraphQL error.
#[async_trait]
pub trait ResolverGuard: Send + Sync + 'static {
    async fn check(&self, ctx: &Context<'_>) -> Result<()>;
}
