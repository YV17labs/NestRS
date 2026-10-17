//! [`authorize`] — the class-level access gate, the GraphQL analog of
//! [`crate::http::Authorize`].

use nest_rs_graphql::async_graphql::{Context, Result};

use super::context::{ability, forbidden, insufficient_scope, unauthenticated};
use crate::gate::{Refusal, transport};
use crate::{ActionMarker, GateVerdict, Subject, gate};

/// Class-level gate: require action `A` on subject `S`. Returns a GraphQL
/// `forbidden` error (code `FORBIDDEN`) when the caller's ability does not grant
/// it, `unauthenticated` (code `UNAUTHENTICATED`) when no principal backs the
/// request, and an error when no ability is present at all (a missing bridge).
///
/// The anonymous caller is refused even when the visitor ability grants `A`:
/// `/graphql` is `#[public]`, and a `define_visitor` grant reaches `#[public]` operations only.
pub fn authorize<A: ActionMarker, S: Subject>(ctx: &Context<'_>) -> Result<()> {
    let ability = ability(ctx)?;
    let verdict = gate::<A, S>(&ability);
    let error = match &verdict {
        GateVerdict::Allowed => return Ok(()),
        GateVerdict::Unauthenticated => unauthenticated(),
        GateVerdict::Forbidden => forbidden(),
        GateVerdict::InsufficientScope(missing) => insufficient_scope(missing),
    };
    crate::gate::warn_denied(Refusal {
        reason: verdict.reason(),
        ..Refusal::of::<A, S>(transport::GRAPHQL)
    });
    Err(error)
}
