//! [`Authorize<A, S>`] — route-level access gate as a poem extractor.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_guards::{Denial, denial_to_http_error};
use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

use crate::gate::{Refusal, transport};
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
/// mechanism) but it is not a posture declaration, and that is the whole
/// reason to go through the decorator: an `#[authorize]` is greppable as one
/// of the three sites an authz decision may live at, a parameter is not.
///
/// Arming is **not** a spelling question. `#[routes]` hands each parameter
/// type to `nest_rs_http::ShaperProbe` and the compiler decides whether it is
/// a `RouteResponseShaper`, so `use ... as Az` arms exactly like the canonical
/// name. What no signature scan can see is an extractor reached *indirectly* —
/// nested in another extractor, or run by a hand-rolled `FromRequest`; that is
/// what the `nest_rs_http::MaskProbe` this extractor marks still backstops,
/// failing such a route closed rather than shipping an unmasked body.
pub struct Authorize<A, S>(PhantomData<fn() -> (A, S)>);

impl<'a, A, S> FromRequest<'a> for Authorize<A, S>
where
    A: ActionMarker,
    S: Subject,
{
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        nest_rs_http::MaskProbe::mark();
        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            // A wiring bug, and the response body is an opaque problem+json: log
            // it or the developer sees a 500 with nothing to grep for.
            tracing::error!(
                target: crate::TARGET,
                action = ?A::ACTION,
                subject = std::any::type_name::<S>(),
                path = %req.original_uri().path(),
                hint = "bind the ability guard (#[use_guards(AuthnGuard, AuthzGuard)]) \
                        and import AuthzModule in this feature's module.rs",
                "missing request Ability — route is authorized but no ability guard ran",
            );
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        if ability.can_class(A::ACTION, TypeId::of::<S>()) {
            return Ok(Authorize(PhantomData));
        }
        // Asked only once the gate has already refused: a withheld rule beside
        // a granted one is not a denial, so `missing_scopes` is the *reason*
        // for this refusal, never a check of its own.
        let missing = ability.missing_scopes(A::ACTION, TypeId::of::<S>());
        if missing.is_empty() {
            // Through the shared emitter, not beside it: three sites wrote this
            // one event and only two carried `transport`, so an operator
            // filtering denials by transport saw every edge except this one.
            crate::gate::warn_denied(Refusal {
                reason: Some(crate::gate::reason::NO_CLASS_GRANT),
                ..Refusal::of::<A, S>(transport::HTTP)
            });
            return Err(denial_to_http_error(Denial::forbidden("forbidden")));
        }
        // A token that verified but is too narrow. The scopes ride to the edge,
        // where the discovery interceptor turns them into the RFC 6750
        // `insufficient_scope` challenge — so the client learns what to ask the
        // authorization server for instead of retrying the same token.
        // The scopes ride on the `Denial`, and the denial line stays the shared
        // one: an event named once is an event queried once.
        crate::gate::warn_denied(Refusal {
            reason: Some(crate::gate::reason::INSUFFICIENT_SCOPE),
            ..Refusal::of::<A, S>(transport::HTTP)
        });
        Err(denial_to_http_error(Denial::insufficient_scope(
            missing,
            "forbidden",
        )))
    }
}
