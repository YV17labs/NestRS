use std::sync::Arc;

use features::Role;
use features::authz::constants::{POSTS_READ, POSTS_WRITE};
use features::sign_in::{ClientPayload, SignInConfig, SignInModule, SignInService};
use nest_rs::authn::{AuthnConfig, AuthnModule, JwtOptions, JwtService};
use nest_rs::config::{Namespaced, var_name};
use nest_rs::core::module;
use nest_rs::http::StatusCode;
use nest_rs::oauth::server::{OAuthServerConfig, RegisteredClient, TokenError};
use nest_rs::seaorm::{SeaOrmConfig, SeaOrmDatabaseModule};
use nest_rs::social::{GithubSocialConfig, GithubSocialModule, SocialRegistry};
use nest_rs::testing::{HeadlessApp, LogCapture, TestAppBuilder};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_00a1);

const RESOURCE: &str = "https://api.example.com";

const SENT_RESOURCE: &str = "https://only-the-caller-knows.example";

const SECRET: &str = "sign-in-grant-test-secret-padded-32";

fn sign_in_service() -> SignInService {
    SignInService::new(
        Arc::new(JwtService::new(JwtOptions::new(SECRET)).expect("jwt service")),
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

    assert!(token.scope.iter().any(|scope| *scope == POSTS_READ));
    assert!(token.scope.iter().any(|scope| *scope == POSTS_WRITE));
}

#[module(
    imports = [
        AuthnModule::for_root(AuthnConfig {
            secret: Some(SECRET.into()),
            ..AuthnConfig::default()
        }),
        GithubSocialModule,
        SignInModule,
        SeaOrmDatabaseModule,
    ],
)]
struct SignInApp;

fn registered(client_id: &str, scopes: &[&str]) -> RegisteredClient<ClientPayload> {
    RegisteredClient::new(
        client_id,
        "s3cr3t-never-logged",
        scopes.iter().copied(),
        [RESOURCE],
        ClientPayload {
            org_id: ORG,
            roles: vec![Role::User],
        },
    )
}

fn sign_in_app(clients: Vec<RegisteredClient<ClientPayload>>) -> TestAppBuilder {
    sign_in_app_serving(vec![RESOURCE.into()], clients)
}

fn sign_in_app_serving(
    resources: Vec<String>,
    clients: Vec<RegisteredClient<ClientPayload>>,
) -> TestAppBuilder {
    HeadlessApp::builder()
        .module::<SignInApp>()
        .provide(sea_orm::DatabaseConnection::default())
        .provide(SeaOrmConfig::default())
        .provide(SignInConfig {
            resources,
            default_org_id: ORG,
        })
        .provide(OAuthServerConfig { clients })
        .provide(GithubSocialConfig {
            client_id: "github-client".into(),
            client_secret: "github-secret".into(),
            redirect_url: "https://auth.example.com/social/github/callback".into(),
            scopes: Vec::new(),
            authorization_parameters: Default::default(),
        })
}

#[tokio::test]
async fn a_client_registered_outside_the_policy_refuses_the_boot_naming_it() {
    let user_shaped = Uuid::now_v7().to_string();
    let clients = var_name(OAuthServerConfig::<ClientPayload>::NAMESPACE, "CLIENTS");

    for (client, said) in [
        (
            registered("ci-runner", &[POSTS_READ.as_str(), "admin"]),
            format!("clients[ci-runner].scopes ({clients}): unknown_scope"),
        ),
        (
            registered(&user_shaped, &[POSTS_READ.as_str()]),
            format!("clients[{user_shaped}].client_id ({clients}): reads_as_a_user"),
        ),
    ] {
        let app = sign_in_app(vec![client])
            .build_headless()
            .await
            .expect("the framework's checks pass");

        let refused = format!("{:#}", app.init().await.expect_err("the policy refuses"));

        assert!(refused.contains(&said), "{refused}");
        assert!(!refused.contains("s3cr3t-never-logged"), "{refused}");
    }
}

#[tokio::test]
async fn a_sign_in_naming_no_resource_refuses_the_boot_with_its_code() {
    let resources = var_name(SignInConfig::NAMESPACE, "RESOURCES");

    let clients = vec![registered("ci-runner", &[POSTS_READ.as_str()])];
    let Err(refused) = sign_in_app_serving(Vec::new(), clients)
        .build_headless()
        .await
    else {
        panic!("a sign-in no resource can be minted for boots");
    };

    let refused = format!("{refused:#}");
    assert!(
        refused.contains(&format!("resources ({resources}): at_least_one_resource")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_social_redirect_that_cannot_be_signed_is_this_servers_own_failure() {
    let app = sign_in_app(vec![registered("ci-runner", &[POSTS_READ.as_str()])])
        .override_value(
            JwtService::new(JwtOptions::eddsa_verify(
                "-----BEGIN PUBLIC KEY-----\n\
                 MCowBQYDK2VwAyEAGb9ECWmEzf6FQbrBZ9w7lshQhqowtrbLDFw4rXAxZuE=\n\
                 -----END PUBLIC KEY-----\n",
            ))
            .expect("a verify-only jwt service"),
        )
        .build_headless()
        .await
        .expect("boots");
    app.init().await.expect("within the policy");
    let svc = app
        .container()
        .get::<SignInService>()
        .expect("SignInModule lists it");

    let Err(refused) = svc.authorize("github", None) else {
        panic!("a transaction no key signs is refused");
    };

    assert!(matches!(refused, TokenError::Sign(_)), "{refused:?}");
    assert_eq!(refused.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
