//! The authenticated caller — what authentication produces and authorization
//! consumes.

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    User,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.roles.contains(&Role::Admin)
    }
}
