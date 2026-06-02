//! [`Authorize<A, S>`] — the route-level access gate, expressed as a poem
//! extractor so it binds to a handler parameter like `Ctx`/`Valid`/`Piped` and
//! needs no macro support.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

use crate::{Ability, ActionMarker, Subject};

/// Declares that a handler requires action `A` on subject `S`. Add it as a
/// parameter — `_authz: Authorize<Read, users::Entity>` — and the request is
/// rejected with `403` unless the request-scoped [`Ability`] grants it.
///
/// The `Ability` is read from the request extensions, where the ability guard
/// placed it (as `Arc<Ability>`). Its absence is a `500`: the guard that builds
/// it was not applied to this route — a wiring bug, not a client error. This is
/// the class-level gate; the per-row filter and the response mask enforce the
/// rule's conditions.
///
/// `#[routes]` also reads this parameter to mask the response transparently, by
/// the type name `Authorize` — so importing it under an alias (`use ... as Foo`)
/// keeps the gate working but silently disables response masking.
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
