//! Covers `src/strategy.rs` and `src/module.rs` — the registry
//! `OAuthServerModule` registers authenticates a client over HTTP Basic,
//! through the strategy and the guard the module provides, and the client it
//! attaches reads what its request is granted.

use base64::Engine as _;
use nest_rs_core::{Ambient, DuplicateProviderError, module};
use nest_rs_http::{HttpModule, StatusCode, controller, routes};
use nest_rs_oauth_server::{
    AccessTokenRequest, AccessTokenResponse, AuthenticatedClient, ClientGuard,
    ClientSecretBasicStrategy, OAuthServerConfig, OAuthServerModule, RegisteredClient,
};
use nest_rs_testing::{LogCapture, TestApp, TestResponse};

const CLIENT_ID: &str = "ci-registered-client";
const SECRET: &str = "s3cret-never-logged";
const API: &str = "https://api.example.com";
const MCP: &str = "https://mcp.example.com/mcp";

#[controller(path = "/")]
struct TokenController;

#[routes]
impl TokenController {
    #[post("/token")]
    #[use_guards(ClientGuard<u32>)]
    async fn token(
        &self,
        client: Ambient<AuthenticatedClient<u32>>,
        request: AccessTokenRequest,
    ) -> nest_rs_http::Result<AccessTokenResponse> {
        let scope = client.scope_for(&request.scope)?;
        let audience = client.audience_for(&request.resource)?;
        Ok(AccessTokenResponse::bearer(format!("for {audience}"), 60).with_scope(scope))
    }
}

fn registry_of<P>(payload: P) -> OAuthServerConfig<P> {
    OAuthServerConfig {
        clients: vec![RegisteredClient::new(
            CLIENT_ID,
            SECRET,
            ["posts:read", "posts:write"],
            [API, MCP],
            payload,
        )],
    }
}

fn registry() -> OAuthServerConfig<u32> {
    registry_of(7)
}

#[module(
    imports = [HttpModule, OAuthServerModule::for_root(registry())],
    providers = [TokenController],
)]
struct IssuerApp;

/// A 6.x composition, listing what the import now provides.
#[module(
    imports = [HttpModule, OAuthServerModule::for_root(registry())],
    providers = [ClientSecretBasicStrategy<u32>, TokenController],
)]
struct ListingIssuerApp;

/// The same, listing the guard the import provides. A payload of its own: a
/// linked `#[module]` answers for what it lists to every app booted beside it,
/// and `IssuerApp` reads `ClientGuard<u32>` from its import.
#[module(
    imports = [HttpModule, OAuthServerModule::for_root(registry_of(7_u64))],
    providers = [ClientGuard<u64>],
)]
struct ListingGuardIssuerApp;

fn basic(secret: &str) -> String {
    let pair = format!("{CLIENT_ID}:{secret}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(pair)
    )
}

async fn token(app: &TestApp, secret: &str, body: &str) -> TestResponse {
    app.http()
        .post("/token")
        .header("authorization", basic(secret))
        .content_type("application/x-www-form-urlencoded")
        .body(body.to_owned())
        .send()
        .await
}

#[tokio::test]
async fn a_registered_client_is_granted_the_scope_and_the_audience_it_asks_for() {
    let app = TestApp::for_module::<IssuerApp>().await.expect("boots");

    let response = token(
        &app,
        SECRET,
        "grant_type=client_credentials&scope=posts%3Aread&resource=https%3A%2F%2Fmcp.example.com%2Fmcp",
    )
    .await;

    response.assert_status_is_ok();
    response
        .assert_json(serde_json::json!({
            "access_token": format!("for {MCP}"),
            "token_type": "Bearer",
            "expires_in": 60,
            "scope": "posts:read",
        }))
        .await;
}

#[tokio::test]
async fn a_wrong_secret_is_refused_by_the_registry_the_module_reads() {
    let app = TestApp::for_module::<IssuerApp>().await.expect("boots");

    let response = token(&app, "wrong-secret", "grant_type=client_credentials").await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response
        .assert_json(serde_json::json!({ "error": "invalid_client" }))
        .await;
}

/// §5.2's `unsupported_grant_type` and `invalid_scope`, and RFC 8707's
/// `invalid_target`, each filed once at `warn` by its reason, the client its
/// actor, never what it asked for.
#[tokio::test]
async fn an_authenticated_clients_refused_request_is_filed_by_its_reason() {
    let app = TestApp::for_module::<IssuerApp>().await.expect("boots");

    for (body, error) in [
        (
            "grant_type=urn%3Anever-logged%3Agrant-type",
            "unsupported_grant_type",
        ),
        (
            "grant_type=client_credentials&scope=posts%3Aread+admin-never-logged",
            "invalid_scope",
        ),
        (
            "grant_type=client_credentials&resource=https%3A%2F%2Fnever-logged.example.com",
            "invalid_target",
        ),
        ("grant_type=client_credentials", "invalid_target"),
    ] {
        let logs = LogCapture::install();
        let response = token(&app, SECRET, body).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        response
            .assert_json(serde_json::json!({ "error": error }))
            .await;
        let event = logs.expect_one("nest_rs::oauth::server", "token request refused");
        assert_eq!(event.level, "warn");
        assert_eq!(event.actor_id.as_deref(), Some(CLIENT_ID));
        assert!(event.fields.keys().eq(["reason"]), "{event:?}");
        assert_eq!(event.field("reason").as_deref(), Some(error));
        let filed = format!("{event:?}");
        assert!(!filed.contains("never-logged"), "{filed}");
        assert!(!filed.contains(SECRET), "{filed}");
    }
}

/// The import registers the strategy, so an app listing it too registers it
/// twice, and the boot names both registrations.
#[tokio::test]
async fn a_strategy_listed_beside_the_import_providing_it_fails_the_boot() {
    let Err(refused) = TestApp::for_module::<ListingIssuerApp>().await else {
        panic!("the strategy would be registered twice");
    };
    let duplicate = refused
        .downcast_ref::<DuplicateProviderError>()
        .unwrap_or_else(|| panic!("not the duplicate: {refused:#}"));
    assert!(
        duplicate.type_name.contains("ClientSecretBasicStrategy"),
        "{duplicate}"
    );
}

/// The guard too: it registers through a hand-written discovery of its own,
/// and listed beside the import it is refused the same way, naming it.
#[tokio::test]
async fn a_guard_listed_beside_the_import_providing_it_fails_the_boot() {
    let Err(refused) = TestApp::for_module::<ListingGuardIssuerApp>().await else {
        panic!("the guard would be registered twice");
    };
    let duplicate = refused
        .downcast_ref::<DuplicateProviderError>()
        .unwrap_or_else(|| panic!("not the duplicate: {refused:#}"));
    assert!(
        duplicate.type_name.contains("ClientAuthenticationGuard"),
        "{duplicate}"
    );
}
