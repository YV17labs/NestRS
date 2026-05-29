use std::sync::Arc;

use nestrs_authz::Create;
use nestrs_authz_http::Authorize;
use nestrs_http::{controller, crud, Ctx, Valid};
use poem::http::StatusCode;
use poem::web::Json;
use poem::{Error, Result};

use identity::Claims;

use crate::authn::AuthGuard;
use crate::authz::AppAbilityGuard;
use crate::users::entity::{self, CreateUserInput, UpdateUserInput, User};
use crate::users::service::UsersService;

#[controller(path = "/users")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct UsersController {
    #[inject]
    svc: Arc<UsersService>,
}

#[crud(
    service = svc,
    entity = entity::Entity,
    output = User,
    create = CreateUserInput,
    update = UpdateUserInput,
)]
impl UsersController {
    #[post("/")]
    #[api(
        summary = "Create a user in the caller's org",
        description = "Requires a bearer JWT (obtain one from `POST /auth/login`). The \
                       user's org is taken from the caller's token, never the body.",
        tags("User")
    )]
    async fn create(
        &self,
        _authz: Authorize<Create, entity::Entity>,
        auth: Ctx<Claims>,
        body: Valid<Json<CreateUserInput>>,
    ) -> Result<Json<User>> {
        let row = self
            .svc
            .create_in_org(body.into_inner(), auth.org_id)
            .await
            .map_err(|err| {
                Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
            })?;
        Ok(Json(User::from(&row)))
    }
}
