use std::sync::Arc;

use nestrs_authz::{Create, Read};
use nestrs_authz_http::{Authorize, Bind};
use nestrs_http::{controller, crud, Ctx, Valid};
use poem::web::Json;
use poem::Result;

use domain::authn::AuthGuard;
use domain::authz::AppAbilityGuard;
use domain::users::{CreateUserInput, Entity as UserEntity, UpdateUserInput, User, UsersService};
use domain::Claims;

#[controller(path = "/users")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct UsersController {
    #[inject]
    svc: Arc<UsersService>,
}

#[crud(
    service = svc,
    entity = UserEntity,
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
        _authz: Authorize<Create, UserEntity>,
        auth: Ctx<Claims>,
        body: Valid<Json<CreateUserInput>>,
    ) -> Result<Json<User>> {
        Ok(Json(
            self.svc.create_in_org(body.into_inner(), auth.org_id).await?,
        ))
    }

    #[get("/:id")]
    #[api(
        summary = "Get a user in the caller's org by id",
        description = "The id is bound to the loaded, authorized user through the \
                       service — a row outside the caller's scope is 403, absent 404.",
        tags("User")
    )]
    async fn get(&self, user: Bind<UsersService, Read>) -> Json<User> {
        Json(User::from(&*user))
    }
}
