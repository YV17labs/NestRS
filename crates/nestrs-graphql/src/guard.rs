//! [`ResolverGuard`] — the GraphQL counterpart of HTTP's `#[use_guards]` guards.
//!
//! NestJS binds `@UseGuards()` uniformly to a controller class **and** a resolver
//! class. The HTTP `Guard` (`nestrs-middleware`) gates a `poem::Request`, which a
//! resolver does not have during execution — it has an [`async_graphql::Context`].
//! So the GraphQL surface keeps its own guard seam, deliberately parallel to the
//! HTTP one (just as `OperationGuard` mirrors `RouteResponseShaper`): a
//! `ResolverGuard` gates a single resolver operation against its context.
//!
//! Bind it with `#[use_guards(GuardA, GuardB)]` on a `#[resolver]` impl block (it
//! runs before *every* operation of that resolver) or on an individual
//! `#[query]`/`#[mutation]`/`#[field]` method (just that one). The macro resolves
//! each guard from the container (the same one resolvers build from), so a guard
//! is an ordinary `#[injectable]` provider that can inject its own dependencies.
//! Returning `Err` short-circuits the operation with that GraphQL error before its
//! body runs — so a guarded operation returns an `async_graphql::Result<_>`.
//!
//! **Division of labour.** Authentication and ability-building stay request-level
//! (the `OperationGuard` bridge runs them once per request and seeds the principal
//! / `Ability` into the context). A `ResolverGuard` is for *authorization* and
//! other per-operation checks that **read** that seeded context — roles, feature
//! flags, maintenance windows — exactly the per-resolver decoration NestJS guards
//! cover beyond the ambient `authorize::<Action, E>` ability gate.
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

/// A guard that runs before a resolver operation and decides whether it proceeds.
/// Returning `Err(error)` short-circuits with that GraphQL error; `Ok(())` lets the
/// operation run. The context carries the per-request state the
/// [`OperationGuard`](crate::OperationGuard) bridge seeded (the caller's principal
/// and `Ability`), which is what an authorization guard reads.
#[async_trait]
pub trait ResolverGuard: Send + Sync + 'static {
    async fn check(&self, ctx: &Context<'_>) -> Result<()>;
}
