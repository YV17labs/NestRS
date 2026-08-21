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
/// # Why the gate also refuses the anonymous caller
///
/// `/graphql` is one endpoint carrying the `Public` marker: the authn guard
/// admits an anonymous caller so `#[public]` *operations* stay reachable, and
/// the ability guard then hands the operation the **visitor** ability
/// ([`AbilityFactory::define_visitor`](crate::AbilityFactory::define_visitor)).
/// Posture is declared per operation instead of per route, and this function is
/// what `#[authorize(...)]` expands to — so it is the site that has to refuse
/// the anonymous caller, exactly as the authn guard on a non-`#[public]` HTTP
/// route refuses one before that route's `Authorize<A, S>` ever runs.
///
/// Without it, a visitor grant added to serve a `#[public]` query would also
/// satisfy every `#[authorize]` operation on the same entity — while the review
/// contract of `define_visitor` is that a grant there reaches `#[public]`
/// surfaces *only*.
pub fn authorize<A: ActionMarker, S: Subject>(ctx: &Context<'_>) -> Result<()> {
    let ability = ability(ctx)?;
    // The decision is `crate::gate`, shared with MCP so `#[authorize]` cannot
    // come to mean two things; only the error frame below is GraphQL's.
    let verdict = gate::<A, S>(&ability);
    let error = match &verdict {
        GateVerdict::Allowed => return Ok(()),
        GateVerdict::Unauthenticated => unauthenticated(),
        GateVerdict::Forbidden => forbidden(),
        // A refusal that a wider token would have fixed says so on this
        // transport too. GraphQL has no `401` for the discovery
        // interceptor to enrich, but a scope refusal is an ordinary error frame
        // here — so the same fact reaches the client, in this transport's own
        // shape.
        GateVerdict::InsufficientScope(missing) => insufficient_scope(missing),
    };
    crate::gate::warn_denied(Refusal {
        reason: verdict.reason(),
        ..Refusal::of::<A, S>(transport::GRAPHQL)
    });
    Err(error)
}
