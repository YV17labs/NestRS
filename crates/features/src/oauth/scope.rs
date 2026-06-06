use crate::Role;

pub fn role_from_db(role: &str) -> Role {
    match role {
        "admin" => Role::Admin,
        _ => Role::User,
    }
}

/// Empty/absent `requested` grants the client's full `allowed` set; an unknown
/// scope yields `None` (rejected upstream as `invalid_scope`).
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
        assert!(matches!(role_from_db("admin"), Role::Admin));
    }

    #[test]
    fn role_from_db_maps_user() {
        assert!(matches!(role_from_db("user"), Role::User));
    }

    #[test]
    fn role_from_db_falls_back_to_user_for_unknown() {
        // Defence in depth: an unrecognised role string never gets escalated to admin.
        assert!(matches!(role_from_db("superuser"), Role::User));
        assert!(matches!(role_from_db(""), Role::User));
    }
}
