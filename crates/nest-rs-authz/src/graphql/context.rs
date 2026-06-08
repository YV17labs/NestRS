//! Per-request [`Ability`] bridge into the GraphQL context. The auth guard
//! chain on `/graphql` stores it on the poem request; the seed forwards it
//! into every GraphQL operation's context.

use std::sync::Arc;

use nest_rs_graphql::GraphqlContextSeed;
use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};

use crate::Ability;

// `owner_type_id: None` because the ambient ability is framework-level — any
// app linking the GraphQL authz bridge wants it forwarded regardless of which
// provider owns the principal. App-specific principal types use
// `forward_principal!`, which is module-gated by the app's auth guard.
nest_rs_graphql::inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || None,
        seed: |req, _container, gql| match req.extensions().get::<Arc<Ability>>() {
            Some(ability) => gql.data(ability.clone()),
            None => gql,
        },
    }
}

/// The request-scoped [`Ability`] in a resolver. Errors if absent — the auth
/// guard chain was not applied to `/graphql`, a wiring bug not a client error.
pub fn ability(ctx: &Context<'_>) -> Result<Arc<Ability>> {
    ctx.data_opt::<Arc<Ability>>().cloned().ok_or_else(|| {
        Error::new("missing request `Ability` — is the GraphQL auth bridge installed on /graphql?")
    })
}

/// A GraphQL `forbidden` error (code `FORBIDDEN`), shared by the gate and `bind`.
pub(crate) fn forbidden() -> Error {
    Error::new("forbidden").extend_with(|_, e| e.set("code", "FORBIDDEN"))
}
