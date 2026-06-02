//! The JWT claims contract: the wire shape the `auth` app signs and every resource
//! server verifies, plus the role enum it carries.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A coarse authorization role carried in the token. Shared so the `auth` app puts
/// the same values in that the `api` app branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

/// The JWT payload the `auth` app issues and every resource server verifies. `exp`
/// is required — a `JwtService` validates it on `verify`, so an expired token is
/// rejected automatically. Keep this stable: it is the cross-app contract.
///
/// A resource server also uses the verified value **as its runtime principal** (the
/// caller), so it carries the access helpers a policy reads — no separate principal
/// type to define until an app needs fields the token does not carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// The org the caller acts within.
    pub org_id: Uuid,
    /// The caller's roles.
    pub roles: Vec<Role>,
    /// Expiry, as a Unix timestamp.
    pub exp: u64,
}

impl Claims {
    /// Whether the caller holds the [`Admin`](Role::Admin) role.
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}
