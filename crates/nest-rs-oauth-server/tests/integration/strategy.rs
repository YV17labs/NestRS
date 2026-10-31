//! Covers `src/strategy.rs` and `src/module.rs` — the registry
//! `OAuthServerModule` registers authenticates a client over HTTP Basic, and
//! the client it attaches reads what its request is granted.

use base64::Engine as _;
use nest_rs_core::{Ctx, module};
use nest_rs_http::{HttpModule, StatusCode, controller, routes};
use nest_rs_oauth_server::{
    AccessTokenRequest, AccessTokenResponse, AuthenticatedClient, ClientAuthenticationGuard,
    ClientSecretBasicStrategy, OAuthServerConfig, OAuthServerModule, RegisteredClient,
};
use nest_rs_testing::{LogCapture, TestApp, TestResponse};

const CLIENT_ID: &str = "ci-registered-client";
const SECRET: &str = "s3cret-never-logged";
const API: &str = "https://api.example.com";
const MCP: &str = "https://mcp.example.com/mcp";

type ClientGuard = ClientAuthenticationGuard<ClientSecretBasicStrategy<u32>>;

#[controller(path = "/")]
struct TokenController;

#[routes]
impl TokenController {
    #[post("/token")]
    #[use_guards(ClientGuard)]
    #[api(response = AccessTokenResponse)]
    async fn token(
        &self,
        client: Ctx<AuthenticatedClient<u32>>,
        request: AccessTokenRequest,
    ) -> nest_rs_http::Result<AccessTokenResponse> {
        let scope = client.scope_for(&request.scope)?;
        let audience = client.audience_for(&request.resource)?;
        Ok(AccessTokenResponse::bearer(format!("for {audience}"), 60).with_scope(scope))
    }
}

#[module(
    imports = [
        HttpModule,
        OAuthServerModule::for_root(OAuthServerConfig {
            clients: vec![RegisteredClient::new(
                CLIENT_ID,
                SECRET,
                ["posts:read", "posts:write"],
                [API, MCP],
                7_u32,
            )],
        }),
    ],
    providers = [ClientSecretBasicStrategy<u32>, ClientGuard, TokenController],
)]
struct IssuerApp;

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
