use std::sync::Arc;

use nest_rs_seaorm::{CrudService, ServiceError};
use nest_rs_ws::{gateway, messages};

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::users::{User, UsersService};

#[gateway(path = "/ws")]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct UsersGateway {
    #[inject]
    svc: Arc<UsersService>,
}

#[messages]
impl UsersGateway {
    #[subscribe_message("users.list")]
    async fn list(&self) -> Result<Vec<User>, ServiceError> {
        let rows = self.svc.list().await?;
        Ok(rows.iter().map(User::from).collect())
    }
}
