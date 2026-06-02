use std::sync::Arc;

use nestrs_database::CrudService;
use nestrs_ws::{gateway, messages};

use crate::authn::AuthGuard;
use crate::authz::ws::WsAuthGuard;
use crate::authz::AppAbilityGuard;
use crate::users::core::{User, UserError, UsersService};

#[gateway(path = "/ws")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct UsersGateway {
    #[inject]
    svc: Arc<UsersService>,
}

#[messages]
impl UsersGateway {
    #[subscribe_message("users.list")]
    #[use_guards(WsAuthGuard)]
    async fn list(&self) -> Result<Vec<User>, UserError> {
        let rows = self.svc.list().await?;
        Ok(rows.iter().map(User::from).collect())
    }
}
