use std::sync::Arc;

use features::Role;
use features::authz::constants::{POSTS_READ, POSTS_WRITE};
use features::oauth::{AuthenticatedClient, ClientPayload, OAuthConfig, OAuthService};
use nest_rs::authn::{JwtOptions, JwtService};
use nest_rs::oauth::server::TokenError;
use nest_rs::social::SocialRegistry;
use nest_rs::testing::{CapturedEvent, LogCapture};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

fn oauth_service() -> OAuthService {
    let jwt_svc = Arc::new(
        JwtService::new(JwtOptions::new("oauth-grant-test-secret-padded-32b"))
            .expect("jwt service"),
    );
    let providers = Arc::new(SocialRegistry::default());
    let users_svc = Arc::new(features::users::UsersService::new(Arc::new(
        DatabaseConnection::default(),
    )));
    let config = Arc::new(OAuthConfig {
        clients: vec![],
        default_org_id: Uuid::nil(),
    });
    OAuthService::new(jwt_svc, providers, users_svc, config)
}

const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00a1);

fn read_only_client() -> AuthenticatedClient {
    AuthenticatedClient {
        payload: ClientPayload {
            org_id: ORG,
            roles: vec![Role::User],
        },
        scopes: vec![POSTS_READ.into()],
    }
}

#[test]
fn grant_client_credentials_rejects_unknown_grant_type() {
    let client = read_only_client();
    let err = oauth_service()
        .grant_client_credentials("password", None, &client)
        .unwrap_err();
    assert!(matches!(err, TokenError::UnsupportedGrant));
}

#[test]
fn grant_client_credentials_rejects_invalid_scope() {
    let client = read_only_client();
    let err = oauth_service()
        .grant_client_credentials("client_credentials", Some(POSTS_WRITE), &client)
        .unwrap_err();
    assert!(matches!(err, TokenError::InvalidScope));
}

#[test]
fn grant_client_credentials_issues_for_valid_scope() {
    let client = read_only_client();
    let token = oauth_service()
        .grant_client_credentials("client_credentials", Some(POSTS_READ), &client)
        .expect("token issued");
    assert_eq!(token.token_type, "Bearer");
    assert!(!token.access_token.is_empty());
}

const SENT_GRANT: &str = "urn:example:grant-only-the-client-knows";
const SENT_SCOPE: &str = "posts:only-the-client-knows";

fn the_one_refusal(logs: &LogCapture) -> CapturedEvent {
    let mut refusals: Vec<CapturedEvent> = logs
        .events()
        .into_iter()
        .filter(|event| event.target == features::oauth::TARGET && event.level == "warn")
        .collect();
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    refusals.remove(0)
}

#[test]
fn an_unsupported_grant_is_logged_by_its_reason_never_by_the_grant_sent() {
    let logs = LogCapture::install();
    let client = read_only_client();

    let refused = oauth_service().grant_client_credentials(SENT_GRANT, None, &client);

    assert!(matches!(refused, Err(TokenError::UnsupportedGrant)));
    let refusal = the_one_refusal(&logs);
    assert_eq!(
        refusal.field("reason").as_deref(),
        Some("unsupported_grant_type")
    );
    assert!(
        refusal
            .fields
            .values()
            .all(|value| !value.contains(SENT_GRANT)),
        "{refusal:?}"
    );
}

#[test]
fn a_refused_scope_is_logged_by_its_reason_never_by_the_scope_sent() {
    let logs = LogCapture::install();
    let client = read_only_client();

    let refused =
        oauth_service().grant_client_credentials("client_credentials", Some(SENT_SCOPE), &client);

    assert!(matches!(refused, Err(TokenError::InvalidScope)));
    let refusal = the_one_refusal(&logs);
    assert_eq!(refusal.field("reason").as_deref(), Some("invalid_scope"));
    assert_eq!(refusal.field("org_id"), Some(ORG.to_string()));
    assert!(
        refusal
            .fields
            .values()
            .all(|value| !value.contains(SENT_SCOPE)),
        "{refusal:?}"
    );
}
