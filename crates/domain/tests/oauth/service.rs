use std::sync::Arc;

use domain::oauth::{AuthenticatedClient, TokenError, TokenIssuer};
use nestrs_authn::{JwtOptions, JwtService};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

fn issuer() -> TokenIssuer {
    let jwt = Arc::new(
        JwtService::new(JwtOptions::new("oauth-grant-test-secret")).expect("jwt service"),
    );
    TokenIssuer::new(jwt, Arc::new(domain::users::UsersService::new(Arc::new(
        DatabaseConnection::default(),
    ))))
}

const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00a1);

#[test]
fn grant_client_credentials_rejects_unknown_grant_type() {
    let client = AuthenticatedClient {
        org_id: ORG,
        scopes: vec!["user".into()],
    };
    let err = issuer()
        .grant_client_credentials("password", None, &client)
        .unwrap_err();
    assert!(matches!(err, TokenError::UnsupportedGrant));
}

#[test]
fn grant_client_credentials_rejects_invalid_scope() {
    let client = AuthenticatedClient {
        org_id: ORG,
        scopes: vec!["user".into()],
    };
    let err = issuer()
        .grant_client_credentials("client_credentials", Some("admin"), &client)
        .unwrap_err();
    assert!(matches!(err, TokenError::InvalidScope));
}

#[test]
fn grant_client_credentials_issues_for_valid_scope() {
    let client = AuthenticatedClient {
        org_id: ORG,
        scopes: vec!["user".into()],
    };
    let token = issuer()
        .grant_client_credentials("client_credentials", Some("user"), &client)
        .expect("token issued");
    assert_eq!(token.token_type, "Bearer");
    assert!(!token.access_token.is_empty());
}
