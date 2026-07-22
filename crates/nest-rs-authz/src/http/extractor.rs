//! [`Authorize<A, S>`] — route-level access gate as a poem extractor.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

use crate::{Ability, ActionMarker, Subject};

/// Enforcement plumbing for action `A` on subject `S`: 403 unless the
/// request-scoped [`Ability`] grants it; 500 when the ability is missing
/// (wiring bug, not a client error). Class-level only — the per-row filter and
/// response mask enforce conditions. Its presence in a handler signature is
/// also what makes `#[routes]` install the response shaper (automatic masking
/// + ambient ability).
///
/// # Don't write this — write `#[authorize(Action, Entity)]`
///
/// The posture of an HTTP route is declared by the decorator, exactly as on a
/// `#[query]`/`#[mutation]`:
///
/// ```rust,ignore
/// #[post("/")]
/// #[authorize(Create, users::Entity)]
/// async fn create(&self, body: Valid<Json<CreateUser>>) -> Result<Json<User>> { … }
/// ```
///
/// `#[routes]` desugars that to this extractor, fully qualified, as the
/// handler's first parameter — the same thing `#[crud]` emits for its
/// generated ops. Writing the parameter by hand still works (it *is* the
/// mechanism) but it is not a posture declaration: `#[routes]` recognises a
/// shaper parameter by **path-segment name**, so a renamed import
/// (`use ... as Az`) silently fails to arm masking. Going through the
/// decorator removes that hazard entirely, and keeps every authz decision
/// greppable as an `#[authorize]` / `#[use_guards]` / `#[public]` site.
///
/// The residual hand-written path is still fail-closed at run time: this
/// extractor marks the route's `nest_rs_http::MaskProbe`, and an unarmed route
/// whose probe was marked fails the request closed (a logged `500`) rather
/// than shipping an unmasked body.
pub struct Authorize<A, S>(PhantomData<fn() -> (A, S)>);

impl<'a, A, S> FromRequest<'a> for Authorize<A, S>
where
    A: ActionMarker,
    S: Subject,
{
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        nest_rs_http::MaskProbe::mark();
        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        if ability.can_class(A::ACTION, TypeId::of::<S>()) {
            Ok(Authorize(PhantomData))
        } else {
            tracing::warn!(
                target: "nest_rs::authz",
                action = ?A::ACTION,
                subject = std::any::type_name::<S>(),
                "authorization denied",
            );
            Err(Error::from_string("forbidden", StatusCode::FORBIDDEN))
        }
    }
}
