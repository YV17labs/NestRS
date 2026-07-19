use std::sync::Arc;

use nest_rs_authz::{Ability, Action, FieldSet, current_ability};
use nest_rs_seaorm::{CrudService, ServiceError};
use nest_rs_ws::{gateway, messages};
use serde_json::Value;

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::users::{Entity as UserEntity, User, UsersService, entities::user};

#[gateway(path = "/users")]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct UsersGateway {
    #[inject]
    svc: Arc<UsersService>,
}

#[messages]
impl UsersGateway {
    #[subscribe_message("users.list")]
    async fn list(&self) -> Result<Vec<Value>, ServiceError> {
        let rows = self.svc.list().await?;
        let ability = current_ability()
            .ok_or_else(|| ServiceError::Masking("no ambient ability for masking".into()))?;
        rows.iter().map(|row| wire_masked(&ability, row)).collect()
    }
}

fn wire_masked(ability: &Ability, row: &user::Model) -> Result<Value, ServiceError> {
    let mut wire =
        serde_json::to_value(User::from(row)).map_err(|e| ServiceError::Masking(e.to_string()))?;
    if let FieldSet::Only(allowed) = ability.permitted_fields::<UserEntity>(Action::Read, row)
        && let Value::Object(map) = &mut wire
    {
        map.retain(|key, _| allowed.contains(key.as_str()));
    }
    Ok(wire)
}
