use std::sync::Arc;

use nest_rs::http::{controller, crud};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::users::UsersService;

#[controller(path = "/users")]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct UsersController {
    #[inject]
    svc: Arc<UsersService>,
}

#[crud(
    service = svc,
    api(
        get(
            summary = "Get a user in the caller's org by id",
            description = "A user outside the caller's org answers 403, an absent one 404."
        ),
        create(
            summary = "Create a user in the caller's org",
            description = "Requires a bearer JWT (obtain one from `POST /login` on the auth app). \
                           The user's org is taken from the caller's token, never the body."
        ),
    ),
)]
impl UsersController {}
