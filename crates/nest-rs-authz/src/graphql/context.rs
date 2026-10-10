//! Per-request [`Ability`] bridge into the GraphQL context. The auth guard
//! chain on `/graphql` stores it on the poem request; the seed forwards it
//! into every GraphQL operation's context.

use std::sync::Arc;

use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use nest_rs_graphql::{GraphqlContextSeed, SeedLifetime};
use nest_rs_guards::Denial;

use crate::Ability;

// `owner_type_id: None`: the ambient ability is framework-level. App principal
// types go through `forward_principal!`, module-gated by the app's auth guard.
nest_rs_core::inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || None,
        // The ability is the caller's, so it lives as long as the connection.
        lifetime: SeedLifetime::Connection,
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
        // Every GraphQL fail-closed exit funnels through here.
        tracing::error!(
            target: crate::TARGET,
            transport = crate::gate::transport::GRAPHQL,
            reason = crate::gate::reason::NO_AMBIENT_ABILITY,
            "authorization denied",
        );
        Error::new("missing request `Ability` — is the GraphQL auth bridge installed on /graphql?")
    })
}

/// A GraphQL `forbidden` error (code `FORBIDDEN`) — the one denial shape every
/// GraphQL refusal carries, whether the class gate or a data-layer `bind`
/// (`nest_rs_seaorm::graphql::bind`) emits it.
pub fn forbidden() -> Error {
    Error::new("forbidden").extend_with(|_, e| e.set("code", "FORBIDDEN"))
}

/// [`forbidden`] naming the response fields the caller's field grant refuses —
/// the answer to an operation that selected a column it may not read. The
/// `fields` extension is a list, so a name containing a comma stays one entry.
pub(crate) fn forbidden_fields(fields: &[String]) -> Error {
    let names = fields.to_vec();
    forbidden().extend_with(move |_, e| e.set("fields", names))
}

/// The refusal a *wider token* would have fixed — code `INSUFFICIENT_SCOPE`,
/// with the scopes to ask the authorization server for in `requiredScopes`.
///
/// Distinct from [`forbidden`] as RFC 6750 §3.1 separates them: this one is
/// actionable. Rendered by `nest_rs_guards::denial_to_graphql_error`, so a gate's
/// refusal and a guard's read identically on the wire.
pub(crate) fn insufficient_scope(required: &[String]) -> Error {
    nest_rs_guards::denial_to_graphql_error(Denial::insufficient_scope(
        required.to_vec(),
        "insufficient_scope",
    ))
}

/// A GraphQL `unauthenticated` error — the anonymous caller's answer to a
/// gated operation. Code `UNAUTHENTICATED`, the same one
/// `nest_rs_guards::denial_to_graphql_error` gives a `401` denial, so a client
/// reads one code for "log in" whichever layer refused.
pub(crate) fn unauthenticated() -> Error {
    Error::new("unauthenticated").extend_with(|_, e| e.set("code", "UNAUTHENTICATED"))
}
