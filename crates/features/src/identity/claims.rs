use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

/// JWT payload signed by the `auth` app and verified by every resource server.
/// Also used as the runtime principal — keep stable, it is the cross-app
/// contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Omitted for machine-only grants (`client_credentials`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<Uuid>,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
    pub exp: u64,
}

impl Claims {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}
