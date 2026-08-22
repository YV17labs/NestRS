use features::Role;
use features::oauth::roles_for_scope;

#[test]
fn empty_scope_grants_all_allowed() {
    let roles = roles_for_scope(None, &["admin".into(), "user".into()]).unwrap();
    assert_eq!(roles, vec![Role::Admin, Role::User]);
}

#[test]
fn unknown_scope_is_rejected() {
    assert!(roles_for_scope(Some("nope"), &["user".into()]).is_none());
}

#[test]
fn subset_scope_resolves_roles() {
    let roles = roles_for_scope(Some("user"), &["admin".into(), "user".into()]).unwrap();
    assert_eq!(roles, vec![Role::User]);
}

#[test]
fn empty_role_list_defaults_to_user() {
    let roles = roles_for_scope(Some("unknown_scope"), &["unknown_scope".into()]).unwrap();
    assert_eq!(roles, vec![Role::User]);
}
