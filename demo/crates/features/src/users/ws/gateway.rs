use std::sync::Arc;

use nest_rs_authz::{Action, masked_reply};
use nest_rs_seaorm::{CrudService, ServiceError};
use nest_rs_ws::{gateway, messages};
use serde_json::Value;

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
    async fn list(&self) -> Result<Value, ServiceError> {
        let rows = self.svc.list().await?;
        let wire = serde_json::to_value(rows.iter().map(User::from).collect::<Vec<_>>())
            .map_err(|e| ServiceError::Masking(e.to_string()))?;
        masked_reply::<UserEntity>(Action::Read, wire)
            .map_err(|e| ServiceError::Masking(e.to_string()))
    }
}
