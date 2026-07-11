//! Resolver-side accessor for request-scoped providers — the GraphQL mirror of
//! [`nest_rs_http::Scoped<T>`]. A framework-level `GraphqlContextSeed`
//! (`context.rs`) forwards the per-request `RequestScope` into the async-graphql
//! context; [`Scoped<T>`] reads it back to resolve an
//! `#[injectable(scope = request)]` provider (or, falling through, a singleton —
//! prefer plain `#[inject]` for those).
//!
//! ```ignore
//! #[query]
//! async fn who(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
//!     let per_req = Scoped::<RequestId>::from_context(ctx)?;
//!     Ok(per_req.value().to_string())
//! }
//! ```
//!
//! **Caveat.** This works inside resolver bodies, which run on the request's
//! task. A `#[dataloader]` batch closure runs off-task (a spawned future), so a
//! request-scoped provider is **not** reachable there — batches re-establish
//! ambient state through their own [`crate::GraphqlBatchContext`] seam.

use std::any::type_name;
use std::ops::Deref;
use std::sync::Arc;

use async_graphql::{Context, Error};
use nest_rs_core::RequestScope;

/// Resolves a provider of type `T` from the current operation's
/// [`RequestScope`]. `from_context` errors if the scope is absent (the schema
/// is not being served over the HTTP transport) or if no provider is registered
/// for `T`.
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Send + Sync + 'static> Scoped<T> {
    /// Resolve `T` from the operation's request scope, forwarded into the
    /// async-graphql context by the framework `GraphqlContextSeed`.
    pub fn from_context(ctx: &Context<'_>) -> async_graphql::Result<Self> {
        let scope = ctx.data::<Arc<RequestScope>>().map_err(|_| {
            Error::new(
                "request scope not installed — serve the schema over the HTTP transport \
                 (GraphqlModule) so RequestScopeEndpoint forwards it",
            )
        })?;
        match scope.get::<T>() {
            Some(value) => Ok(Scoped(value)),
            None => Err(Error::new(format!(
                "no provider registered for `{}` — add it to a module's providers",
                type_name::<T>()
            ))),
        }
    }
}
