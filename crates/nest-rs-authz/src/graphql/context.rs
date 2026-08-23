//! Per-request [`Ability`] bridge into the GraphQL context. The auth guard
//! chain on `/graphql` stores it on the poem request; the seed forwards it
//! into every GraphQL operation's context.

use std::sync::Arc;

use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use nest_rs_graphql::{GraphqlContextSeed, SeedLifetime};
use nest_rs_guards::Denial;

use crate::Ability;

// `owner_type_id: None` because the ambient ability is framework-level — any
// app linking the GraphQL authz bridge wants it forwarded regardless of which
// provider owns the principal. App-specific principal types use
// `forward_principal!`, which is module-gated by the app's auth guard.
nest_rs_graphql::inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || None,
        // The ability is the caller's, so a socket carries it exactly as long
        // as it carries the caller — bounded by the connection ceiling, not by
        // one operation.
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
        // Say so to the operator, exactly as the other three edges do. Every
        // GraphQL fail-closed exit funnels through here — the gate and both
        // `masked_*_for` — so without this line a broken `AuthzGraphqlModule`
        // showed up only as error frames in client responses, with nothing on
        // `nest_rs::authz` to find. HTTP, WS and MCP each log this at `error`
        // with a machine reason; this was the fourth.
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
/// the answer to an operation that selected a column it may not read. The names
/// ride as an extension rather than in the message so the message stays the one
/// constant string every denial carries.
///
/// The extension is a **list**, which is the natural reading of "names in the
/// `fields` extension" and the only shape that survives more than one refused
/// field: a comma-joined string forced every client to re-split it, and a
/// client that read it as an array broke. Takes the names structurally so a
/// field containing a comma cannot split into two entries.
pub(crate) fn forbidden_fields(fields: &[String]) -> Error {
    let names = fields.to_vec();
    forbidden().extend_with(move |_, e| e.set("fields", names))
}

/// The refusal a *wider token* would have fixed — code `INSUFFICIENT_SCOPE`,
/// with the scopes to ask the authorization server for in `requiredScopes`.
///
/// Distinct from [`forbidden`] for the reason RFC 6750 §3.1 separates them on
/// HTTP: a plain `forbidden` is final, this one is actionable, and a client that
/// cannot tell them apart either gives up too early or retries forever.
///
/// Rendered *by* `nest_rs_guards::denial_to_graphql_error` rather than beside
/// it: an operation refused by the gate and one refused by a guard must read
/// identically on the wire, and two renderers agreeing today is not that
/// guarantee.
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
