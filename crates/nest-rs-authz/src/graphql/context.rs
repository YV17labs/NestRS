//! The [`Ability`] a GraphQL field runs under — the one its unit carries,
//! which its class gate checked — and GraphQL's denial shape.

use std::sync::Arc;

use nest_rs_core::problem::code;
use nest_rs_core::{Edge, Problem};
use nest_rs_graphql::async_graphql::{Error, ErrorExtensions, Result};
use nest_rs_guards::Denial;

use crate::Ability;
use crate::gate::Refusal;

/// The [`Ability`] the running field's unit carries — what the guards at its
/// request or at the field itself attached, the one its class gate checked. A
/// wiring defect when absent — no ability guard ran — refused opaquely and
/// filed at `error`, never a client error.
pub fn ability() -> Result<Arc<Ability>> {
    crate::current_ability().ok_or_else(|| {
        // Every GraphQL fail-closed exit funnels through here.
        crate::gate::error_unreached(Refusal {
            remedy: Some(UNREACHED),
            ..Refusal::on(Edge::Graphql.as_str())
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
