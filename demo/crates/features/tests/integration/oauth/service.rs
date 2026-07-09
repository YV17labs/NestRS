use std::sync::Arc;

use features::oauth::{AuthenticatedClient, IssuerConfig, OAuthService};
use nest_rs_authn::{JwtOptions, JwtService, OAuth2Client, OAuth2Config, TokenError};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

fn oauth_service() -> OAuthService {
    let jwt_svc =
        Arc::new(JwtService::new(JwtOptions::new("oauth-grant-test-secret")).expect("jwt service"));
    let oauth = Arc::new(
        OAuth2Client::new(OAuth2Config {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://auth.example/oauth/authorize".into(),
            token_url: "https://auth.example/oauth/token".into(),
            redirect_url: "https://app.example/oauth/callback".into(),
            userinfo_url: "https://auth.example/oauth/userinfo".into(),
            scopes: vec!["read".into()],
        })
        .expect("oauth client"),
    );
    let users_svc = Arc::new(features::users::UsersService::new(Arc::new(
        DatabaseConnection::default(),
    )));
    let config = Arc::new(IssuerConfig {
        clients: vec![],
        default_org_id: Uuid::nil(),
    });
    OAuthService::new(jwt_svc, oauth, users_svc, config)
}

const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00a1);

#[test]
fn grant_client_credentials_rejects_unknown_grant_type() {
    let client = AuthenticatedClient {
        payload: ORG,
        scopes: vec!["user".into()],
    };
    let err = oauth_service()
        .grant_client_credentials("password", None, &client)
        .unwrap_err();
    assert!(matches!(err, TokenError::UnsupportedGrant));
}

#[test]
fn grant_client_credentials_rejects_invalid_scope() {
    let client = AuthenticatedClient {
        payload: ORG,
        scopes: vec!["user".into()],
    };
    let err = oauth_service()
        .grant_client_credentials("client_credentials", Some("admin"), &client)
        .unwrap_err();
    assert!(matches!(err, TokenError::InvalidScope));
}

#[test]
fn grant_client_credentials_issues_for_valid_scope() {
    let client = AuthenticatedClient {
        payload: ORG,
        scopes: vec!["user".into()],
    };
    let token = oauth_service()
        .grant_client_credentials("client_credentials", Some("user"), &client)
        .expect("token issued");
    assert_eq!(token.token_type, "Bearer");
    assert!(!token.access_token.is_empty());
}
