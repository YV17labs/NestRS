use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

use crate::authz::AuthzGuard;
use crate::users::{CreateUser, Entity as UserEntity, UpdateUser, User, UsersService};

#[resolver]
#[use_guards(AuthzGuard)]
pub struct UsersResolver {
    #[inject]
    svc: Arc<UsersService>,
}

#[crud(
    service = svc,
    entity = UserEntity,
    output = User,
    create = CreateUser,
    update = UpdateUser,
)]
impl UsersResolver {}
