//! [`authorize`] — the class-level access gate, the GraphQL analog of
//! [`crate::http::Authorize`].

use std::any::TypeId;

use nest_rs_graphql::async_graphql::{Context, Error, Result};

use super::context::{ability, forbidden, insufficient_scope, unauthenticated};
use crate::{ActionMarker, Subject};

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
    // Authentication first, and separately: an anonymous caller is refused for
    // want of a principal, whatever the visitor branch granted.
    if ability.is_visitor() {
        return Err(deny::<A, S>("anonymous_caller", unauthenticated()));
    }
    if ability.can_class(A::ACTION, TypeId::of::<S>()) {
        return Ok(());
    }
    // A refusal that a wider token would have fixed says so, on this transport
    // too. GraphQL has no `401` for the resource-server interceptor to enrich,
    // but a scope refusal is an ordinary error frame here — so the same fact
    // reaches the client, in this transport's native shape.
    let missing = ability.missing_scopes(A::ACTION, TypeId::of::<S>());
    if missing.is_empty() {
        return Err(deny::<A, S>("no_class_grant", forbidden()));
    }
    Err(deny::<A, S>(
        "insufficient_scope",
        insufficient_scope(&missing),
    ))
}

/// The one `warn` every gate refusal passes through, so a denial cannot reach
/// the client without leaving the queryable trace an incident is answered from.
fn deny<A: ActionMarker, S: Subject>(reason: &'static str, error: Error) -> Error {
    tracing::warn!(
        target: "nest_rs::authz",
        transport = "graphql",
        action = ?A::ACTION,
        subject = std::any::type_name::<S>(),
        reason,
        "authorization denied",
    );
    error
}
