use std::sync::Arc;

use nest_rs_authz::{Read, masked_output_ambient};
use nest_rs_seaorm::{CrudService, ServiceError};
use nest_rs_ws::{gateway, messages};

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::users::{Entity as UserEntity, User, UsersService};

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
        rows.iter()
            .map(|row| {
                masked_output_ambient::<Read, UserEntity, User>(row)
                    .map_err(|err| ServiceError::Masking(err.to_string()))
            })
            .collect()
    }
}
