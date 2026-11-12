//! Covers `src/guard.rs` — a client a token endpoint refuses reads RFC 6749
//! §5.2's answer, by the method it attempted, whatever route binds the guard.
//!
//! Booted, because the refusal must survive the guard stage and the transport:
//! a resource server's interceptor must not stamp its pointer onto it.

use std::time::Duration;

use base64::Engine as _;
use nest_rs_authn::{AuthError, AuthnConfig, AuthnModule, Strategy, basic_credentials};
use nest_rs_config::Surface;
use nest_rs_core::{Ambient, async_trait, injectable, module};
use nest_rs_http::{HttpModule, Json, Request, StatusCode, controller, routes};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
use nest_rs_oauth_server::{
    AccessTokenRequest, AuthenticatedClient, ClientAuthenticationGuard, RegisteredClient,
    TokenError, authenticate_against_registry,
};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule};
use nest_rs_testing::{LogCapture, TestApp, TestResponse};
use serde_json::Value;

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

/// A body the edge decodes, on a route a client's guard stands in front of.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Declared {
    name: String,
}

#[controller(path = "/")]
struct TokenController;

#[routes]
impl TokenController {
    #[post("/token")]
    #[use_guards(Authenticated)]
    #[api(error(500 = TokenError))]
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

    #[post("/token/declared")]
    #[use_guards(Authenticated)]
    async fn declared(&self, Json(declared): Json<Declared>) -> nest_rs_http::Result<String> {
        Ok(declared.name)
    }

    #[post("/token/request")]
    #[use_guards(Authenticated)]
    async fn request(&self, request: AccessTokenRequest) -> nest_rs_http::Result<String> {
        Ok(request.grant_type.to_string())
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

/// The same endpoint, documented: its document reads what the wire answers.
#[module(
    imports = [
        HttpModule,
        OpenApiModule::for_root(OpenApiConfig {
            enabled: Surface::Open,
            ..OpenApiConfig::default()
        }),
    ],
    providers = [TokenController, Registry, Outage, Authenticated, Unavailable],
)]
struct DocumentedTokenApp;

/// RFC 6749 §2.3.1 and §5.2 in the document: a generated client authenticates
/// with `Basic` and reads the `error` member of every refusal the guard writes.
#[tokio::test]
async fn the_document_says_basic_and_the_body_every_refusal_answers() {
    let app = TestApp::for_module::<DocumentedTokenApp>()
        .await
        .expect("boots");

    let response = app.http().get("/api-json").send().await;

    response.assert_status_is_ok();
    let doc: Value = response.json().await.value().deserialize();
    let operation = &doc["paths"]["/token"]["post"];
    assert_eq!(
        operation["security"],
        serde_json::json!([{ "basicAuth": [] }]),
        "{operation}",
    );
    let scheme = &doc["components"]["securitySchemes"]["basicAuth"];
    assert_eq!(scheme["type"], "http", "{scheme}");
    assert_eq!(scheme["scheme"], "basic", "{scheme}");
    for status in ["400", "401", "503"] {
        let answer = &operation["responses"][status]["content"];
        assert_eq!(
            answer.get("application/problem+json").is_some(),
            status == "503",
            "the edge's request deadline alone answers a problem here: {status}: {answer}",
        );
        let schema = &answer["application/json"]["schema"];
        let reference = schema["$ref"].as_str().expect("a named schema");
        let name = reference.rsplit('/').next().expect("its name");
        let body = &doc["components"]["schemas"][name];
        assert_eq!(body["required"], serde_json::json!(["error"]), "{body}");
        let codes = body["properties"]["error"]["enum"]
            .as_array()
            .expect("the codes §5.2 registers");
        assert!(codes.contains(&Value::from("invalid_client")), "{body}");
        assert!(codes.contains(&Value::from("server_error")), "{body}");
    }
    let challenge = operation["responses"]["401"]["headers"]["WWW-Authenticate"]["description"]
        .as_str()
        .expect("a 401 names its challenge");
    assert!(challenge.contains("Basic"), "{challenge}");
    assert!(!challenge.contains("Bearer"), "{challenge}");
    let server_error = &operation["responses"]["500"]["content"];
    assert!(
        server_error.get("application/json").is_some(),
        "what the handler declares sits beside the opaque 500: {server_error}"
    );
    assert!(
        server_error.get("application/problem+json").is_some(),
        "{server_error}"
    );
    assert_eq!(
        doc["paths"]["/token/public"]["post"]["security"],
        serde_json::json!([{ "basicAuth": [] }]),
        "`#[public]` opens nothing the guard holds shut",
    );
}

/// The guard's words do not hide the edge's on one status: a body the edge
/// decodes is refused in a problem document, the client in §5.2's, each on a
/// `400`, and the document lists both.
#[tokio::test]
async fn a_guards_body_sits_beside_the_problem_the_edge_answers_on_its_status() {
    let app = TestApp::for_module::<DocumentedTokenApp>()
        .await
        .expect("boots");

    let response = app.http().get("/api-json").send().await;
    let doc: Value = response.json().await.value().deserialize();
    let listed = &doc["paths"]["/token/declared"]["post"]["responses"]["400"]["content"];
    assert!(listed.get("application/json").is_some(), "{listed}");
    assert!(listed.get("application/problem+json").is_some(), "{listed}");

    let undecoded = app
        .http()
        .post("/token/declared")
        .header("authorization", basic(CLIENT_ID, SECRET))
        .content_type("application/json")
        .body("{no json")
        .send()
        .await;
    undecoded.assert_status(StatusCode::BAD_REQUEST);
    undecoded.assert_header("content-type", "application/problem+json");

    let refused = post(&app, "/token/declared", None).await;
    refused.assert_status(StatusCode::BAD_REQUEST);
    refused.assert_header("content-type", "application/json");
}

/// Where the request itself refuses in §5.2's words, as the guard does, the
/// `400` is those words alone.
#[tokio::test]
async fn a_request_refusing_as_its_guard_does_lists_one_body_on_its_400() {
    let app = TestApp::for_module::<DocumentedTokenApp>()
        .await
        .expect("boots");

    let response = app.http().get("/api-json").send().await;
    let doc: Value = response.json().await.value().deserialize();
    let listed = &doc["paths"]["/token/request"]["post"]["responses"]["400"]["content"];
    assert!(listed.get("application/json").is_some(), "{listed}");
    assert!(listed.get("application/problem+json").is_none(), "{listed}");
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
