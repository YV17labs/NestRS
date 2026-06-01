use std::sync::Arc;

use nestrs_database::CrudService;
use nestrs_ws::{gateway, messages};

use crate::authn::AuthGuard;
use crate::authz::AppAbilityGuard;
use crate::users::entity::User;
use crate::users::service::UsersService;

#[gateway(path = "/ws")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct UsersGateway {
    #[inject]
    svc: Arc<UsersService>,
}

#[messages]
impl UsersGateway {
    #[subscribe_message("users.list")]
    async fn list(&self) -> Vec<User> {
        match self.svc.list().await {
            Ok(rows) => rows.iter().map(User::from).collect(),
            Err(err) => {
                tracing::error!(target: "nestrs::ws", error = %err, "users.list query failed");
                Vec::new()
            }
        }
    }
}
