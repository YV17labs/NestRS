//! Per-request [`Ability`] bridge into the GraphQL context. The auth guard
//! chain on `/graphql` attaches it; the seed forwards it into every GraphQL
//! operation's context.

use std::sync::Arc;

use nest_rs_core::Problem;
use nest_rs_core::problem::code;
use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use nest_rs_graphql::{GraphqlContextSeed, SeedLifetime};
use nest_rs_guards::Denial;

use crate::Ability;
use crate::gate::Refusal;

// `owner_type_id: None`: the ambient ability is framework-level. App principal
// types go through `forward_principal!`, module-gated by the app's auth guard.
nest_rs_core::inventory::submit! {
    GraphqlContextSeed {
        owner_type_id: || None,
        // The ability is the caller's, so it lives as long as the connection.
        lifetime: SeedLifetime::Connection,
        seed: |_req, _container, gql| match crate::current_ability() {
            Some(ability) => gql.data(ability),
            None => gql,
        },
    }
}

/// The request-scoped [`Ability`] in a resolver. A wiring defect when absent —
/// no ability guard ran on `/graphql`'s request — refused opaquely and filed
/// at `error`, never a client error.
pub fn ability(ctx: &Context<'_>) -> Result<Arc<Ability>> {
    ctx.data_opt::<Arc<Ability>>().cloned().ok_or_else(|| {
        // Every GraphQL fail-closed exit funnels through here.
        crate::gate::error_unreached(Refusal {
            remedy: Some(UNREACHED),
            ..Refusal::on(crate::gate::transport::GRAPHQL)
        });
        nest_rs_graphql::denial_to_graphql_error(Denial::internal(
            "no ability reached the GraphQL operation",
        ))
    })
}

/// What an app whose `/graphql` attaches no ability is told to change.
const UNREACHED: &str = "attach the ability on /graphql's request — the app's global guard \
                         pool or EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)> — and \
                         import AuthzModule";

/// A GraphQL `forbidden` error (code `FORBIDDEN`) — the one denial shape every
/// GraphQL refusal carries, whether the class gate or a data-layer `bind`
/// (`nest_rs_seaorm::graphql::bind`) emits it.
pub fn forbidden() -> Error {
    nest_rs_graphql::problem_error(&Problem::new(403, code::FORBIDDEN))
}

/// [`forbidden`] naming the response fields the caller's field grant refuses —
/// the answer to an operation that selected a column it may not read. The
/// `fields` extension is a list, so a name containing a comma stays one entry.
pub(crate) fn forbidden_fields(fields: &[String]) -> Error {
    let names = fields.to_vec();
    forbidden().extend_with(move |_, e| e.set("fields", names))
}
