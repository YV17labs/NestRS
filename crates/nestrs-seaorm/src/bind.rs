//! [`Bind<S, A>`] — route-model binding for HTTP routes: turn a path id into
//! the loaded, authorized entity. Outcomes: bad UUID → 400, absent → 404,
//! denied → 403 (existence intentionally not hidden), else the loaded model.
//!
//! Loads through the entity's service ([`CrudService::access`]), never the ORM
//! directly, so a by-id binding emits the same `nestrs::orm` access span as
//! every other data access.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use nestrs_authz::{Ability, ActionMarker, with_ability};
use nestrs_core::RequestScope;
use poem::http::StatusCode;
use poem::web::Path;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::{Access, CrudService};

/// The loaded, authorized entity bound from a path id, through service `S`.
/// Declare as a handler parameter (`user: Bind<UsersService, Read>`); read the
/// model via [`Deref`] or own it with [`into_inner`](Bind::into_inner).
pub struct Bind<S: CrudService, A>(<S::Entity as EntityTrait>::Model, PhantomData<fn() -> A>);

impl<S: CrudService, A> Bind<S, A> {
    pub fn into_inner(self) -> <S::Entity as EntityTrait>::Model {
        self.0
    }
}

impl<S: CrudService, A> Deref for Bind<S, A> {
    type Target = <S::Entity as EntityTrait>::Model;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S, A> FromRequest<'a> for Bind<S, A>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let Path(id) = Path::<Uuid>::from_request(req, body).await?;
        if id.get_version_num() != 7 {
            return Err(Error::from_string(
                "path id must be a UUID v7",
                StatusCode::BAD_REQUEST,
            ));
        }

        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        let scope = req.extensions().get::<Arc<RequestScope>>().ok_or_else(|| {
            Error::from_string(
                "request scope not installed — RequestScopeEndpoint must wrap the route tree",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        let service = scope.get::<S>().ok_or_else(|| {
            Error::from_string(
                "no provider registered for the bound service — add it to a module's providers",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        let access = with_ability(ability.clone(), service.access(A::ACTION, id))
            .await
            .map_err(|err| {
                Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
            })?;
        match access {
            Access::Found(model) => Ok(Bind(model, PhantomData)),
            Access::Denied => Err(Error::from_status(StatusCode::FORBIDDEN)),
            Access::Missing => Err(Error::from_status(StatusCode::NOT_FOUND)),
        }
    }
}
