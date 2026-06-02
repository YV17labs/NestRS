//! [`authorize`] — the class-level access gate, the GraphQL analog of
//! [`crate::http::Authorize`].

use std::any::TypeId;

use nestrs_graphql::async_graphql::{Context, Result};

use super::context::{ability, forbidden};
use crate::{ActionMarker, Subject};

/// Class-level gate: require action `A` on subject `S`. Returns a GraphQL
/// `forbidden` error (code `FORBIDDEN`) when the caller's ability does not grant
/// it (or when no ability is present — so it doubles as the auth gate).
pub fn authorize<A: ActionMarker, S: Subject>(ctx: &Context<'_>) -> Result<()> {
    if ability(ctx)?.can_class(A::ACTION, TypeId::of::<S>()) {
        Ok(())
    } else {
        Err(forbidden())
    }
}
