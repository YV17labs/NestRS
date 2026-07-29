//! [`authorize`] — the class-level access gate, the GraphQL analog of
//! [`crate::http::Authorize`].

use std::any::TypeId;

use nest_rs_graphql::async_graphql::{Context, Error, Result};

use super::context::{ability, forbidden, unauthenticated};
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
    let (reason, denial) = if ability.is_visitor() {
        ("anonymous_caller", unauthenticated as fn() -> Error)
    } else if ability.can_class(A::ACTION, TypeId::of::<S>()) {
        return Ok(());
    } else {
        ("no_class_grant", forbidden as fn() -> Error)
    };
    tracing::warn!(
        target: "nest_rs::authz",
        transport = "graphql",
        action = ?A::ACTION,
        subject = std::any::type_name::<S>(),
        reason,
        "authorization denied",
    );
    Err(denial())
}
