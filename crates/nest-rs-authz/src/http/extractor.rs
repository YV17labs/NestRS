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
/// **The parameter is load-bearing even bound as `_authz`** — it is the
/// route's masking declaration, not dead code: its presence in the signature
/// is what makes `#[routes]` install the response shaper (automatic response
/// masking + ambient ability). Deleting the "unused" parameter disarms both.
/// The GraphQL analog is `#[authorize(Action, Entity)]` on a
/// `#[query]`/`#[mutation]`.
///
/// `#[routes]` installs the response shaper (masking + ambient ability) by
/// **textually** matching a handler-parameter type whose path has a segment
/// literally named `Authorize` or `Bind` — so `nest_rs_authz::http::Authorize`
/// or any module-qualified path works, but a **renamed** alias
/// (`use ... as Az`) does **not**: the class-level gate still runs, but masking
/// and the ambient-ability install are silently skipped. The miss is
/// fail-closed (a request-scoped executor with no ambient ability denies every
/// row via `scope_for`), so the route degrades to "no data", never a leak —
/// but prefer a qualified path over a rename to keep masking active.
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
