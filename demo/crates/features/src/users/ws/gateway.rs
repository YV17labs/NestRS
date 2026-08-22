use std::sync::Arc;

use nest_rs::authz::Read;
use nest_rs::seaorm::{CrudService, ServiceError};
use nest_rs::ws::{gateway, messages};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::users::{Entity as UserEntity, User, UsersService};

#[gateway(path = "/users")]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct UsersGateway {
    #[inject]
    svc: Arc<UsersService>,
}

#[messages]
impl UsersGateway {
    #[subscribe_message("users.list")]
    #[authorize(Read, UserEntity)]
    async fn list(&self) -> Result<Vec<User>, ServiceError> {
        Ok(self.svc.list().await?.iter().map(User::from).collect())
    }
}
