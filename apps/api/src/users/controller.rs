use std::sync::Arc;

use nestrs_authz::{Ability, Action, Authorize, Create, Read};
use nestrs_http::{controller, routes, Ctx, Piped, Valid};
use nestrs_pipes::ParseUuidV7;
use poem::http::StatusCode;
use poem::web::{Json, Path};
use poem::{Error, Result};

use crate::auth::{AuthGuard, AuthUser};
use crate::authz::AbilityGuard;
use crate::users::entity::{self, CreateUserInput};
use crate::users::service::UsersService;

#[controller(path = "/users")]
pub struct UsersController {
    #[inject]
    svc: Arc<UsersService>,
}

#[routes]
impl UsersController {
    #[get("/")]
    #[use_guards(AuthGuard, AbilityGuard)]
    #[api(summary = "List users in the caller's org", tags("Users"))]
    async fn list(
        &self,
        _authz: Authorize<Read, entity::Entity>,
        ability: Ctx<Arc<Ability>>,
    ) -> Result<Json<Vec<entity::Model>>> {
        let scope = ability.condition_for::<entity::Entity>(Action::Read);
        Ok(Json(self.svc.list(scope).await.map_err(internal)?))
    }

    #[get("/:id")]
    #[use_guards(AuthGuard, AbilityGuard)]
    #[api(
        summary = "Fetch a user by id (scoped to the caller's org)",
        tags("Users")
    )]
    async fn get(
        &self,
        _authz: Authorize<Read, entity::Entity>,
        ability: Ctx<Arc<Ability>>,
        id: Piped<ParseUuidV7, Path<String>>,
    ) -> Result<Json<entity::Model>> {
        match self.svc.find(id.into_inner()).await.map_err(internal)? {
            Some(row) if ability.can::<entity::Entity>(Action::Read, &row) => Ok(Json(row)),
            // Exists but outside the caller's org: 403, not 404.
            Some(_) => Err(Error::from_status(StatusCode::FORBIDDEN)),
            None => Err(Error::from_status(StatusCode::NOT_FOUND)),
        }
    }

    #[post("/")]
    #[use_guards(AuthGuard, AbilityGuard)]
    #[api(
        summary = "Create a user in the caller's org",
        description = "Requires the `x-api-key` and `x-org-id` headers.",
        tags("Users")
    )]
    async fn create(
        &self,
        _authz: Authorize<Create, entity::Entity>,
        auth: Ctx<AuthUser>,
        body: Valid<Json<CreateUserInput>>,
    ) -> Result<Json<entity::Model>> {
        let row = self
            .svc
            .create(body.into_inner(), auth.org_id)
            .await
            .map_err(internal)?;
        Ok(Json(row))
    }
}

fn internal(err: impl std::fmt::Display) -> Error {
    Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}
