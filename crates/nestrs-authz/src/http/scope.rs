//! [`Scope<E, A>`] — the caller's row-level filter as a handler argument, for a
//! handler that builds its own query. Prefer letting `Repo` scope itself; reach
//! for `Scope` only when a custom query is unavoidable.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use sea_orm::sea_query::Condition;
use sea_orm::EntityTrait;

use crate::{Ability, ActionMarker};

/// The row-level [`Condition`] the caller may apply for action `A` on `E`.
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
