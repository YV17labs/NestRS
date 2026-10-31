//! Covers `src/guard.rs` — a client a token endpoint refuses reads RFC 6749
//! §5.2's answer, by the method it attempted, whatever route binds the guard.
//!
//! Booted, because the refusal must survive the guard stage and the transport:
//! a resource server's interceptor must not stamp its pointer onto it.

use std::time::Duration;

use base64::Engine as _;
use nest_rs_authn::{AuthError, AuthnConfig, AuthnModule, Strategy, basic_credentials};
use nest_rs_core::{Ambient, async_trait, injectable, module};
use nest_rs_http::{HttpModule, Request, StatusCode, controller, routes};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
use nest_rs_oauth_server::{
    AuthenticatedClient, ClientAuthenticationGuard, RegisteredClient, authenticate_against_registry,
};
use nest_rs_testing::{LogCapture, TestApp, TestResponse};

const CLIENT_ID: &str = "ci-registered-client";
const SECRET: &str = "s3cret-never-logged";
const UNKNOWN_ID: &str = "ghost-unregistered-client";
const WRONG_SECRET: &str = "wrong-secret-never-logged";

type Client = AuthenticatedClient<u32>;

/// RFC 6749 §2.3.1's HTTP Basic, against a registry of one client.
#[injectable]
#[derive(Default)]
struct Registry;

#[async_trait]
impl Strategy for Registry {
    type Principal = Client;

    async fn authenticate(&self, req: &mut Request) -> Result<Client, AuthError> {
        let (client_id, client_secret) =
            basic_credentials(req.headers()).ok_or(AuthError::MissingCredentials)?;
        let registry = [RegisteredClient::new(
            CLIENT_ID,
            SECRET,
            ["posts:read"],
            ["https://api.example.com"],
            7,
        )];
        authenticate_against_registry(&registry, &client_id, &client_secret)
    }
}

/// A client store that did not answer, and said when to come back.
#[injectable]
#[derive(Default)]
struct Outage;

#[async_trait]
impl Strategy for Outage {
    type Principal = Client;

    async fn authenticate(&self, _req: &mut Request) -> Result<Client, AuthError> {
        Err(AuthError::unavailable(
            "the client store did not answer",
            Some(Duration::from_secs(30)),
        ))
    }
}

type Authenticated = ClientAuthenticationGuard<Registry>;
type Unavailable = ClientAuthenticationGuard<Outage>;

#[controller(path = "/")]
struct TokenController;

#[routes]
impl TokenController {
    #[post("/token")]
    #[use_guards(Authenticated)]
    async fn token(&self, client: Ambient<Client>) -> nest_rs_http::Result<String> {
        let actor = nest_rs_core::current_actor_id().unwrap_or_default();
        Ok(format!("{} {} {actor}", client.client_id, client.payload))
    }

    #[post("/token/public")]
    #[public]
    #[use_guards(Authenticated)]
    async fn public(&self) -> nest_rs_http::Result<String> {
        Ok("issued".to_owned())
    }

    #[post("/token/outage")]
    #[use_guards(Unavailable)]
    async fn outage(&self) -> nest_rs_http::Result<String> {
        Ok("issued".to_owned())
    }
}

#[module(
    imports = [HttpModule],
    providers = [TokenController, Registry, Outage, Authenticated, Unavailable],
)]
struct TokenApp;

const RESOURCE: &str = "http://localhost";

/// The same token endpoint in an app that is a resource server too: the RFC
/// 9728 interceptor runs around every HTTP unit.
#[module(
    imports = [
        HttpModule,
        AuthnModule::for_root(AuthnConfig {
            secret: Some("resource-server-secret-0123456789".into()),
            ..AuthnConfig::default()
        }),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some(RESOURCE.into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..Default::default()
        }),
    ],
    providers = [TokenController, Registry, Outage, Authenticated, Unavailable],
)]
struct ResourceAndTokenApp;

fn basic(client_id: &str, secret: &str) -> String {
    let pair = format!("{client_id}:{secret}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(pair)
    )
}

async fn post(app: &TestApp, path: &str, authorization: Option<&str>) -> TestResponse {
    let request = app
        .http()
        .post(path)
        .content_type("application/x-www-form-urlencoded");
    let request = match authorization {
        Some(value) => request.header("authorization", value),
        None => request,
    };
    request.body("grant_type=client_credentials").send().await
}

/// What a client reads back: the status, the headers a client acts on and
/// the body's bytes.
async fn read(response: TestResponse) -> (StatusCode, Vec<(String, String)>, String) {
    let status = response.0.status();
    let mut headers: Vec<(String, String)> = response
        .0
        .headers()
        .iter()
        .filter(|(name, _)| {
            [
                "www-authenticate",
                "content-type",
                "cache-control",
                "pragma",
                "retry-after",
            ]
            .contains(&name.as_str())
        })
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().expect("ASCII").to_owned(),
            )
        })
        .collect();
    headers.sort();
    let body = response.0.into_body().into_string().await.expect("a body");
    (status, headers, body)
}

fn invalid_client_over_basic() -> (StatusCode, Vec<(String, String)>, String) {
    (
        StatusCode::UNAUTHORIZED,
        vec![
            ("cache-control".to_owned(), "no-store".to_owned()),
            ("content-type".to_owned(), "application/json".to_owned()),
            ("pragma".to_owned(), "no-cache".to_owned()),
            (
                "www-authenticate".to_owned(),
                r#"Basic realm="authorization server""#.to_owned(),
            ),
        ],
        r#"{"error":"invalid_client"}"#.to_owned(),
    )
}

fn invalid_client_without_authentication() -> (StatusCode, Vec<(String, String)>, String) {
    (
        StatusCode::BAD_REQUEST,
        vec![
            ("cache-control".to_owned(), "no-store".to_owned()),
            ("content-type".to_owned(), "application/json".to_owned()),
            ("pragma".to_owned(), "no-cache".to_owned()),
        ],
        r#"{"error":"invalid_client"}"#.to_owned(),
    )
}

/// RFC 6749 §5.2: a client that authenticated through the `Authorization`
/// header is answered `401` with the scheme it used, the body §5.2's JSON.
#[tokio::test]
async fn a_wrong_secret_over_basic_is_a_401_challenged_with_basic() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    let response = post(&app, "/token", Some(&basic(CLIENT_ID, WRONG_SECRET))).await;

    assert_eq!(read(response).await, invalid_client_over_basic());
}

/// No `Authorization` header — credentials in the body, `client_secret_post`,
/// which this endpoint does not serve, or none — is §5.2's own `400`, with no
/// challenge a browser would answer with a credentials prompt.
#[tokio::test]
async fn a_client_that_did_not_use_the_header_is_a_400_carrying_no_challenge() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    let none = post(&app, "/token", None).await;
    assert_eq!(read(none).await, invalid_client_without_authentication());

    let in_body = app
        .http()
        .post("/token")
        .content_type("application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=client_credentials&client_id={CLIENT_ID}&client_secret={SECRET}"
        ))
        .send()
        .await;
    assert_eq!(read(in_body).await, invalid_client_without_authentication());
}

/// Whatever the header says, the client used it: another scheme, or a
/// `Basic` that does not decode, is answered with the scheme this server reads.
#[tokio::test]
async fn another_scheme_or_an_undecodable_basic_is_the_401_challenged_with_basic() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    for authorization in ["Bearer not-a-client-credential", "Basic !!not-base64!!"] {
        let response = post(&app, "/token", Some(authorization)).await;
        assert_eq!(
            read(response).await,
            invalid_client_over_basic(),
            "{authorization}"
        );
    }
}

/// An unknown client and a known one with a wrong secret read the same bytes.
#[tokio::test]
async fn an_unknown_client_and_a_wrong_secret_answer_the_same_bytes() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    let unknown = post(&app, "/token", Some(&basic(UNKNOWN_ID, SECRET))).await;
    let wrong = post(&app, "/token", Some(&basic(CLIENT_ID, WRONG_SECRET))).await;

    assert_eq!(read(unknown).await, read(wrong).await);
}

/// Every client authenticates at a token endpoint (§3.2.1), so `#[public]`
/// opens nothing: the guard refuses there as anywhere.
#[tokio::test]
async fn a_public_route_refuses_an_unauthenticated_client_all_the_same() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    let none = post(&app, "/token/public", None).await;
    assert_eq!(read(none).await, invalid_client_without_authentication());

    let wrong = post(&app, "/token/public", Some(&basic(CLIENT_ID, WRONG_SECRET))).await;
    assert_eq!(read(wrong).await, invalid_client_over_basic());
}

/// A client store that did not answer decided nothing: `503`, the opaque
/// code, and the wait the store gave.
#[tokio::test]
async fn an_unavailable_strategy_is_a_503_server_error_with_its_retry_after() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    let response = post(&app, "/token/outage", Some(&basic(CLIENT_ID, SECRET))).await;

    assert_eq!(
        read(response).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            vec![
                ("cache-control".to_owned(), "no-store".to_owned()),
                ("content-type".to_owned(), "application/json".to_owned()),
                ("pragma".to_owned(), "no-cache".to_owned()),
                ("retry-after".to_owned(), "30".to_owned()),
            ],
            r#"{"error":"server_error"}"#.to_owned(),
        )
    );
}

/// The client admitted is the request's principal: the handler reads it
/// back, and it is the audit identity, as RFC 9068 §2.2 names a client.
#[tokio::test]
async fn an_admitted_client_is_read_back_as_the_actor() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    let response = post(&app, "/token", Some(&basic(CLIENT_ID, SECRET))).await;

    response.assert_status_is_ok();
    response
        .assert_text(format!("{CLIENT_ID} 7 {CLIENT_ID}"))
        .await;
}

/// One refusal, one `warn`, the edge's on the guard's target: the method
/// attempted and the reason, never the client's id nor its secret.
#[tokio::test]
async fn each_refusal_files_one_warn_naming_neither_the_client_nor_its_secret() {
    let app = TestApp::for_module::<TokenApp>().await.expect("boots");

    for (authorization, method, reason) in [
        (
            Some(basic(UNKNOWN_ID, WRONG_SECRET)),
            "client_secret_basic",
            "failed",
        ),
        (None, "none", "missing_credentials"),
    ] {
        let logs = LogCapture::install();
        let response = post(&app, "/token", authorization.as_deref()).await;
        drop(read(response).await);

        let said: Vec<_> = logs
            .events()
            .into_iter()
            .filter(|event| event.level == "warn" || event.level == "error")
            .collect();
        let [refusal] = said.as_slice() else {
            panic!("one line per refusal: {said:#?}");
        };
        assert_eq!(refusal.target, nest_rs_oauth_server::TARGET);
        assert_eq!(refusal.message, "guard denied the request");
        assert!(
            refusal
                .field("error")
                .is_some_and(|chain| chain.contains(&format!("`{method}`"))),
            "{refusal:#?}",
        );
        assert_eq!(refusal.field("reason").as_deref(), Some(reason));
        let printed = format!("{refusal:?}");
        for secret in [UNKNOWN_ID, WRONG_SECRET] {
            assert!(!printed.contains(secret), "{printed}");
        }
    }
}

/// In an app that is also a resource server, a client's refusal stays §5.2's:
/// no RFC 9728 pointer joins the `Basic` challenge, and the `400` carries none.
#[tokio::test]
async fn a_resource_servers_interceptor_leaves_the_refusal_as_it_is() {
    let app = TestApp::for_module::<ResourceAndTokenApp>()
        .await
        .expect("boots");

    let over_basic = post(&app, "/token", Some(&basic(CLIENT_ID, WRONG_SECRET))).await;
    assert_eq!(read(over_basic).await, invalid_client_over_basic());

    let none = post(&app, "/token", None).await;
    assert_eq!(read(none).await, invalid_client_without_authentication());
}
