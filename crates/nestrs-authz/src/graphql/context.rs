//! The per-request [`Ability`] bridge into the GraphQL context.
//!
//! An HTTP guard chain (run on `/graphql` by the app's GraphQL auth bridge) builds
//! the caller's [`Ability`] and stores it on the poem request. Linking this module
//! registers a [`ContextSeed`] that forwards that `Arc<Ability>` into every GraphQL
//! operation's context, where [`ability`] reads it back and
//! [`authorize`](super::authorize) / [`bind`](super::bind) gate on it.

use std::sync::Arc;

use nestrs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use nestrs_graphql::ContextSeed;

use crate::Ability;

// Forward the request-scoped `Arc<Ability>` (placed on the request by the auth
// guard chain) into every GraphQL operation's context.
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
/// ([`mask`](Ability::mask)) layers. Errors if absent — the auth guard chain was
/// not applied to `/graphql`, a wiring bug, not a client error.
pub fn ability(ctx: &Context<'_>) -> Result<Arc<Ability>> {
    ctx.data_opt::<Arc<Ability>>().cloned().ok_or_else(|| {
        Error::new("missing request `Ability` — is the GraphQL auth bridge installed on /graphql?")
    })
}

/// A GraphQL `forbidden` error (code `FORBIDDEN`), shared by the gate and `bind`.
pub(crate) fn forbidden() -> Error {
    Error::new("forbidden").extend_with(|_, e| e.set("code", "FORBIDDEN"))
}
