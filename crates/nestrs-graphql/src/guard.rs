//! [`ResolverGuard`] — distinct from HTTP's `Guard` because a resolver has no
//! `poem::Request`, it has an [`async_graphql::Context`]. Authentication and
//! ability-building stay request-level (the `OperationGuard` bridge seeds the
//! principal / `Ability` into the context); a `ResolverGuard` reads that
//! seeded state to gate one operation.
//!
//! ```ignore
//! #[nestrs_core::injectable]
//! #[derive(Default)]
//! struct AdminOnly;
//!
//! #[nestrs_graphql::async_trait]
//! impl nestrs_graphql::ResolverGuard for AdminOnly {
//!     async fn check(&self, ctx: &nestrs_graphql::async_graphql::Context<'_>)
//!         -> nestrs_graphql::async_graphql::Result<()> {
//!         match ctx.data_opt::<Principal>() {
//!             Some(p) if p.is_admin() => Ok(()),
//!             _ => Err(nestrs_graphql::async_graphql::Error::new("forbidden")),
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
