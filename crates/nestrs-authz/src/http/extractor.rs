//! [`Authorize<A, S>`] — route-level access gate as a poem extractor.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

use crate::{Ability, ActionMarker, Subject};

/// Declares that a handler requires action `A` on subject `S`:
/// `_authz: Authorize<Read, users::Entity>`. 403 unless the request-scoped
/// [`Ability`] grants it; 500 when the ability is missing (wiring bug, not a
/// client error). Class-level only — the per-row filter and response mask
/// enforce conditions.
///
/// `#[routes]` reads this parameter **by type name** to install the response
/// shaper: importing it under an alias (`use ... as Foo`) keeps the gate
/// working but silently disables response masking.
pub struct Authorize<A, S>(PhantomData<fn() -> (A, S)>);

impl<'a, A, S> FromRequest<'a> for Authorize<A, S>
where
    A: ActionMarker,
    S: Subject,
{
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        if ability.can_class(A::ACTION, TypeId::of::<S>()) {
            Ok(Authorize(PhantomData))
        } else {
            Err(Error::from_string("forbidden", StatusCode::FORBIDDEN))
        }
    }
}
