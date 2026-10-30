use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

use crate::authz::AuthzGuard;
use crate::users::UsersService;

#[resolver]
#[use_guards(AuthzGuard)]
pub struct UsersResolver {
    #[inject]
    svc: Arc<UsersService>,
}

#[crud(service = svc)]
impl UsersResolver {}
