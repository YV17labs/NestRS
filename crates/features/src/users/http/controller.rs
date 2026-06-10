use std::sync::Arc;

use nest_rs_authz::http::Authorize;
use nest_rs_authz::{Create, Read};
use nest_rs_http::{Ctx, Valid, controller, crud};
use nest_rs_seaorm::Bind;
use poem::Result;
use poem::web::Json;

use crate::Claims;
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::users::{CreateUserInput, Entity as UserEntity, UpdateUserInput, User, UsersService};

#[controller(path = "/users")]
#[use_guards(AuthGuard, AuthzGuard)]
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
        description = "Requires a bearer JWT (obtain one from `POST /login` on the auth app). The \
                       user's org is taken from the caller's token, never the body.",
        tags("User")
    )]
    async fn create(
        &self,
        _authz: Authorize<Create, UserEntity>,
        auth: Ctx<Claims>,
        body: Valid<Json<CreateUserInput>>,
    ) -> Result<Json<User>> {
        let user = self
            .svc
            .create_in_org(body.into_inner(), auth.org_id)
            .await?;
        Ok(Json(User::from(&user)))
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
