use crate::Role;
use crate::users::UserRole;

pub fn role_from_db(role: UserRole) -> Role {
    match role {
        UserRole::Admin => Role::Admin,
        UserRole::User => Role::User,
    }
}

pub fn roles_for_scope(requested: Option<&str>, allowed: &[String]) -> Option<Vec<Role>> {
    let granted: Vec<&str> = match requested {
        Some(raw) if !raw.trim().is_empty() => {
            let requested: Vec<&str> = raw.split_whitespace().collect();
            if requested
                .iter()
                .any(|s| !allowed.iter().any(|grant| grant == s))
            {
                return None;
            }
            requested
        }
        _ => allowed.iter().map(String::as_str).collect(),
    };
    let roles: Vec<Role> = granted
        .iter()
        .filter_map(|scope| match *scope {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        })
        .collect();
    Some(if roles.is_empty() {
        vec![Role::User]
    } else {
        roles
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_from_db_maps_admin() {
        assert!(matches!(role_from_db(UserRole::Admin), Role::Admin));
    }

    #[test]
    fn role_from_db_maps_user() {
        assert!(matches!(role_from_db(UserRole::User), Role::User));
    }
}
