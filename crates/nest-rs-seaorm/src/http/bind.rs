//! [`Bind<A, S>`] — route-model binding for HTTP routes: turn a path id into
//! the loaded, authorized entity. Outcomes: bad UUID → 400, absent → 404,
//! denied → 403 (existence intentionally not hidden), else the loaded model.
//!
//! Loads through the entity's service ([`CrudService::access`]), never the ORM
//! directly, so a by-id binding emits the same `nest_rs::orm` access span as
//! every other data access.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use nest_rs_authz::{Ability, ActionMarker, with_ability};
use nest_rs_core::RequestScope;
use poem::http::StatusCode;
use poem::web::Path;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::error::log_by_id_load_failure;
use crate::{Access, CrudService, ServiceError};

/// The loaded, authorized entity bound from a path id, through service `S`.
/// Declare as a handler parameter (`user: Bind<Read, UsersService>`); read the
/// model via [`Deref`] or own it with [`into_inner`](Bind::into_inner).
pub struct Bind<A, S: CrudService>(<S::Entity as EntityTrait>::Model, PhantomData<fn() -> A>);

impl<A, S: CrudService> Bind<A, S> {
    /// Take ownership of the loaded, authorized model.
    pub fn into_inner(self) -> <S::Entity as EntityTrait>::Model {
        self.0
    }
}

impl<A, S: CrudService> Deref for Bind<A, S> {
    type Target = <S::Entity as EntityTrait>::Model;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, A, S> FromRequest<'a> for Bind<A, S>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        nest_rs_http::MaskProbe::mark();
        let Path(id) = Path::<Uuid>::from_request(req, body).await?;
        if id.get_version_num() != 7 {
            // Same wording as the `#[crud]`-generated gate
            // (`nest_rs_codegen::UUID_V7_REQUIRED`), which this crate cannot
            // name: codegen is a macro-time helper, not a runtime dependency.
            return Err(Error::from_string(
                "id must be a UUID v7",
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

        // A failed load logs in full and ships the crate's one opaque DbErr
        // envelope (`ServiceError::Db` — problem+json 500, constant detail),
        // so SQL/driver text never reaches the client.
        let access = with_ability(ability.clone(), service.access(A::ACTION, id))
            .await
            .map_err(|err| {
                log_by_id_load_failure(std::any::type_name::<S>(), &err);
                Error::from(ServiceError::Db(err))
            })?;
        match access {
            Access::Found(model) => Ok(Bind(model, PhantomData)),
            Access::Denied => Err(Error::from_status(StatusCode::FORBIDDEN)),
            Access::Missing => Err(Error::from_status(StatusCode::NOT_FOUND)),
        }
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::DbErr;

    use super::*;

    #[tokio::test]
    async fn db_errors_map_to_an_opaque_500_with_no_driver_text() {
        let err = DbErr::Custom("connection to db.internal:5432 refused".into());
        let resp = Error::from(ServiceError::Db(err)).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = resp.into_body().into_string().await.expect("body");
        assert!(
            !body.contains("db.internal"),
            "driver text must not reach the client: {body}"
        );
    }
}
