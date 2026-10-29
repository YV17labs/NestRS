use std::sync::Arc;

use features::Role;
use features::authz::constants::{POSTS_READ, POSTS_WRITE};
use features::sign_in::{ClientPayload, SignInConfig, SignInService};
use nest_rs::authn::{JwtOptions, JwtService};
use nest_rs::oauth::server::{OAuthServerConfig, TokenError};
use nest_rs::social::SocialRegistry;
use nest_rs::testing::LogCapture;
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00a1);

const RESOURCE: &str = "https://api.example.com";

const SENT_RESOURCE: &str = "https://only-the-caller-knows.example";

fn sign_in_service() -> SignInService {
    SignInService::new(
        Arc::new(
            JwtService::new(JwtOptions::new("sign-in-grant-test-secret-padded-32"))
                .expect("jwt service"),
        ),
        Arc::new(SocialRegistry::default()),
        Arc::new(features::users::UsersService),
        Arc::new(SignInConfig {
            resources: vec![RESOURCE.into()],
            default_org_id: ORG,
        }),
        Arc::new(OAuthServerConfig::<ClientPayload>::default()),
    )
}

#[test]
fn a_sign_in_for_a_resource_it_does_not_serve_is_logged_by_its_reason_never_by_the_resource() {
    let logs = LogCapture::install();

    let refused =
        sign_in_service().issue(Some(SENT_RESOURCE), Uuid::now_v7(), ORG, vec![Role::User]);

    assert!(matches!(refused, Err(TokenError::InvalidTarget)));
    let refusal = logs.expect_one(features::sign_in::TARGET, "sign-in refused");
    assert_eq!(refusal.field("reason").as_deref(), Some("invalid_target"));
    assert!(
        refusal
            .fields
            .values()
            .all(|value| !value.contains(SENT_RESOURCE)),
        "{refusal:?}"
    );
}

#[test]
fn a_social_authorize_for_a_resource_it_does_not_serve_is_refused_before_any_provider() {
    let refused = sign_in_service().authorize("github", Some(SENT_RESOURCE));

    assert!(matches!(refused, Err(TokenError::InvalidTarget)));
}

#[test]
fn a_first_party_sign_in_is_delegated_every_scope_of_the_policy() {
    let token = sign_in_service()
        .issue(None, Uuid::now_v7(), ORG, vec![Role::Admin])
        .expect("issued");

    assert!(token.scope.iter().any(|scope| scope == POSTS_READ));
    assert!(token.scope.iter().any(|scope| scope == POSTS_WRITE));
}
