//! [`Scope<E, A>`] — the caller's row-level filter as a handler argument.
//!
//! The Tier-1, explicit counterpart to `nestrs-database`'s transparent `Repo`
//! scoping: a handler that runs a *custom* query takes `Scope<E, A>` and passes
//! the [`Condition`] to its service, instead of fishing the ability out of
//! `Ctx<Arc<Ability>>` and calling `condition_for` by hand. The condition is the
//! one the caller's [`Ability`] permits for action `A` on entity `E`
//! ([`Ability::condition_for`]); with the framework's `Repo`, prefer letting the
//! read scope itself and reach for `Scope` only when you build the query yourself.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use nestrs_authz::{Ability, ActionMarker};
use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use sea_orm::sea_query::Condition;
use sea_orm::EntityTrait;

/// The row-level [`Condition`] the caller may apply for action `A` on entity `E`.
/// Read it via [`Deref`] or own it with [`into_inner`](Scope::into_inner) to pass
/// to a service query.
pub struct Scope<E, A>(Condition, PhantomData<fn() -> (E, A)>);

impl<E, A> Scope<E, A> {
    pub fn into_inner(self) -> Condition {
        self.0
    }
}

impl<E, A> Deref for Scope<E, A> {
    type Target = Condition;
    fn deref(&self) -> &Condition {
        &self.0
    }
}

impl<'a, E, A> FromRequest<'a> for Scope<E, A>
where
    E: EntityTrait,
    A: ActionMarker,
{
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        Ok(Scope(ability.condition_for::<E>(A::ACTION), PhantomData))
    }
}
