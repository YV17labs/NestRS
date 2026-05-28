//! GraphQL surface for [`nestrs-authz`](nestrs_authz) — the analog of
//! `nestrs-authz-http`'s `Authorize` extractor, for resolvers.
//!
//! An HTTP guard (`nestrs_authz_http::AbilityGuard`, bound **globally** so it
//! runs on `/graphql`) builds the actor's [`Ability`] and stores it on the
//! request. Linking this crate registers a [`ContextSeed`] that forwards that
//! `Arc<Ability>` into the GraphQL context, where a resolver gates on it:
//!
//! ```ignore
//! use nestrs_authz::Read;
//! use nestrs_authz_graphql::authorize;
//!
//! #[resolver]
//! impl UsersResolver {
//!     #[query]
//!     async fn users(&self, ctx: &Context<'_>) -> Result<Vec<User>> {
//!         authorize::<Read, users::Entity>(ctx)?; // class gate, mirrors HTTP `Authorize<Read, _>`
//!         let scope = ability(ctx)?.condition_for::<users::Entity>(Action::Read);
//!         // ...query with `scope`, then `ability.mask_many(...)` the rows.
//!     }
//! }
//! ```
//!
//! Because the gate runs on the request's `Ability`, the HTTP ability guard must
//! be applied to `/graphql` (globally, since `GraphqlModule` self-mounts it).

use std::any::TypeId;
use std::sync::Arc;

use nestrs_authz::{Ability, ActionMarker, Subject};
use nestrs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use nestrs_graphql::ContextSeed;

// Forward the request-scoped `Arc<Ability>` (placed on the request by the HTTP
// ability guard) into every GraphQL operation's context.
nestrs_graphql::inventory::submit! {
    ContextSeed {
        seed: |req, _container, gql| match req.extensions().get::<Arc<Ability>>() {
            Some(ability) => gql.data(ability.clone()),
            None => gql,
        },
    }
}

/// The request-scoped [`Ability`] in a resolver, for the per-row
/// ([`condition_for`](Ability::condition_for)) and field-mask
/// ([`mask`](Ability::mask)) layers. Errors if absent — the HTTP ability guard
/// was not applied to `/graphql`, a wiring bug, not a client error.
pub fn ability(ctx: &Context<'_>) -> Result<Arc<Ability>> {
    ctx.data_opt::<Arc<Ability>>().cloned().ok_or_else(|| {
        Error::new(
            "missing request `Ability` — bind the HTTP ability guard globally so it runs on /graphql",
        )
    })
}

/// Class-level gate: require action `A` on subject `S`, the GraphQL analog of
/// `nestrs_authz_http::Authorize<A, S>`. Returns a GraphQL `forbidden` error
/// (code `FORBIDDEN`) when the actor's ability does not grant it.
pub fn authorize<A: ActionMarker, S: Subject>(ctx: &Context<'_>) -> Result<()> {
    if ability(ctx)?.can_class(A::ACTION, TypeId::of::<S>()) {
        Ok(())
    } else {
        Err(Error::new("forbidden").extend_with(|_, e| e.set("code", "FORBIDDEN")))
    }
}
