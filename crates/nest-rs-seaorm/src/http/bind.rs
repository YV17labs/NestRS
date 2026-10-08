use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use nest_rs_authz::{Ability, ActionMarker, with_ability};
use poem::http::StatusCode;
use poem::web::Path;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::{Access, CrudService, ServiceError};

/// The loaded, authorized entity bound from a path id, through service `S`.
/// Declare as a handler parameter (`user: Bind<Read, UsersService>`); read the
/// model via [`Deref`] or own it with [`into_inner`](Bind::into_inner).
///
/// A non-v7 id answers 400, an absent row 404, a denied one 403 (existence is
/// not hidden).
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
            return Err(Error::from_string(
                nest_rs_core::UUID_V7_REQUIRED,
                StatusCode::BAD_REQUEST,
            ));
        }

        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        let scope = nest_rs_http::current_request_scope().ok_or_else(|| {
            Error::from_string(
                "request scope not installed — the transport edge must wrap the route tree",
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
            .map_err(|err| Error::from(ServiceError::Db(err)))?;
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
