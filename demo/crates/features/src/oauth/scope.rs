use nest_rs::oauth::server::TokenError;

use crate::Role;
use crate::users::UserRole;

pub fn role_from_db(role: UserRole) -> Role {
    match role {
        UserRole::Admin => Role::Admin,
        UserRole::User => Role::User,
    }
}

pub(crate) fn granted_scopes(
    requested: Option<&str>,
    registered: &[String],
) -> Result<Vec<String>, TokenError> {
    let Some(requested) = requested.filter(|raw| !raw.trim().is_empty()) else {
        return Ok(registered.to_vec());
    };
    let mut granted: Vec<String> = Vec::new();
    for scope in requested.split_whitespace() {
        if !registered.iter().any(|own| own == scope) {
            return Err(TokenError::InvalidScope);
        }
        if !granted.iter().any(|kept| kept == scope) {
            granted.push(scope.to_owned());
        }
    }
    Ok(granted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::constants::{AUDIO_TRANSCODE, POSTS_READ, POSTS_WRITE};

    #[test]
    fn role_from_db_maps_admin() {
        assert!(matches!(role_from_db(UserRole::Admin), Role::Admin));
    }

    #[test]
    fn role_from_db_maps_user() {
        assert!(matches!(role_from_db(UserRole::User), Role::User));
    }

    fn registered() -> Vec<String> {
        vec![POSTS_READ.to_owned(), POSTS_WRITE.to_owned()]
    }

    #[test]
    fn an_absent_or_blank_request_is_granted_the_registered_scopes() {
        assert_eq!(granted_scopes(None, &registered()).unwrap(), registered());
        assert_eq!(
            granted_scopes(Some("  "), &registered()).unwrap(),
            registered()
        );
    }

    #[test]
    fn a_registered_subset_is_granted_as_requested() {
        assert_eq!(
            granted_scopes(Some(POSTS_READ), &registered()).unwrap(),
            [POSTS_READ]
        );
    }

    #[test]
    fn a_repeated_scope_is_granted_once() {
        let requested = format!("{POSTS_READ} {POSTS_READ}");
        assert_eq!(
            granted_scopes(Some(&requested), &registered()).unwrap(),
            [POSTS_READ]
        );
    }

    #[test]
    fn a_scope_outside_the_registration_is_refused_whole() {
        let requested = format!("{POSTS_READ} {AUDIO_TRANSCODE}");
        assert!(matches!(
            granted_scopes(Some(&requested), &registered()),
            Err(TokenError::InvalidScope)
        ));
    }
}
