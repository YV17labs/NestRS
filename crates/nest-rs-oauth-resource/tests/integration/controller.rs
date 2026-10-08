//! Covers `src/controller.rs`: the RFC 9728 discovery flow it serves,
//! walked end to end by a client that knows only one thing: the URL of a
//! protected route.
//!
//! ```text
//! GET /posts            → 401 + WWW-Authenticate: Bearer resource_metadata="…"
//! GET <resource_metadata>  → the RFC 9728 document
//! GET <authorization_servers[0]>/.well-known/oauth-authorization-server
//!                       → the AS metadata, whose `issuer` the client validates
//! ```

use crate::AlwaysUnauthorized;
use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_core::module;
use nest_rs_http::{controller, routes};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule, WELL_KNOWN_PATH};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use serde_json::Value;

/// This deployment's canonical URI: the loopback origin, as `TestClient` speaks paths.
const RESOURCE: &str = "http://localhost";
/// The advertised authorization server, served by the stub below; its path
/// component makes a client walk the RFC 8414 §3.1 priority order.
const ISSUER: &str = "http://localhost/stub-as";
/// 32 bytes: HS256's floor, enforced in `JwtService::new`.
const SECRET: &str = "discovery-flow-secret-0123456789";
/// The one scope this deployment advertises.
const SCOPE: &str = "posts:read";

#[controller(path = "/posts")]
#[use_guards(AlwaysUnauthorized)]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "never reached without a token"
    }
}

/// Stands in for the authorization server's metadata endpoint, answering only the
/// third URL of the discovery order, so a client must fall through the first two.
#[controller(path = "/stub-as")]
struct StubAuthorizationServer;

#[routes]
impl StubAuthorizationServer {
    #[get("/.well-known/openid-configuration")]
    #[public]
    async fn metadata(&self) -> poem::web::Json<Value> {
        poem::web::Json(serde_json::json!({
            "issuer": ISSUER,
            "authorization_endpoint": format!("{ISSUER}/oauth/authorize"),
            "token_endpoint": format!("{ISSUER}/oauth/token"),
            "code_challenge_methods_supported": ["S256"],
        }))
    }
}

fn authn() -> AuthnConfig {
    AuthnConfig {
        secret: Some(SECRET.into()),
        // The audience `OAuthResourceModule` requires: the resource identifier.
        audience: Some(RESOURCE.into()),
        ..AuthnConfig::default()
    }
}

fn resource() -> OAuthResourceConfig {
    OAuthResourceConfig::default()
        .with_resource(RESOURCE)
        .with_authorization_servers([ISSUER])
        .with_scopes_supported([SCOPE])
}

#[module(
    imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root(resource()),
    ],
    providers = [PostsController, StubAuthorizationServer, AlwaysUnauthorized],
)]
struct DiscoveryApp;

/// Pull a quoted parameter out of a `WWW-Authenticate` value.
fn challenge_param(challenge: &str, name: &str) -> Option<String> {
    let start = challenge.find(&format!("{name}=\""))? + name.len() + 2;
    let rest = &challenge[start..];
    Some(rest[..rest.find('"')?].to_owned())
}

/// The path of an advertised absolute URL, once it is shown to live under `origin`.
fn path_of(absolute: &str, origin: &str) -> String {
    assert!(
        absolute.starts_with(origin),
        "advertised URL `{absolute}` must live under the resource origin `{origin}`",
    );
    absolute[origin.len()..].to_owned()
}

#[tokio::test]
async fn a_client_walks_from_a_401_to_the_authorization_server() {
    let app = TestApp::for_module::<DiscoveryApp>()
        .await
        .expect("the discovery app boots");

    // Hop 1 — the client knows only the protected route.
    let denied = app.http().get("/posts").send().await;
    denied.assert_status(StatusCode::UNAUTHORIZED);
    let challenge = crate::challenge(&denied.0);

    assert_eq!(challenge_param(&challenge, "scope").as_deref(), Some(SCOPE));

    // Hop 2 — follow `resource_metadata` rather than guessing the well-known path.
    let metadata_url = challenge_param(&challenge, "resource_metadata")
        .expect("the challenge must point at the metadata document");
    assert_eq!(
        metadata_url,
        format!("{RESOURCE}{WELL_KNOWN_PATH}"),
        "the advertised URL is the RFC 9728 well-known path at the resource origin",
    );

    let document = app
        .http()
        .get(path_of(&metadata_url, RESOURCE))
        .send()
        .await;
    document.assert_status_is_ok();
    let document: Value =
        serde_json::from_slice(&document.0.into_body().into_bytes().await.expect("a body"))
            .expect("the metadata document is JSON");

    assert_eq!(
        document["resource"], RESOURCE,
        "the document must name the resource the client will put in its RFC 8707 `resource` parameter",
    );
    let issuer = document["authorization_servers"][0]
        .as_str()
        .expect("RFC 9728 requires at least one authorization server")
        .to_owned();

    // Hop 3 — the issuer has a path, so the three well-known URLs are tried in the
    // order the spec fixes.
    let (origin, tenant) = issuer.split_at(
        issuer["https://".len()..]
            .find('/')
            .map(|i| i + "https://".len())
            .expect("this test's issuer carries a path"),
    );
    let candidates = [
        format!("{origin}/.well-known/oauth-authorization-server{tenant}"),
        format!("{origin}/.well-known/openid-configuration{tenant}"),
        format!("{issuer}/.well-known/openid-configuration"),
    ];
    let mut as_document = None;
    for candidate in &candidates {
        let resp = app.http().get(path_of(candidate, origin)).send().await;
        if resp.0.status() != StatusCode::OK {
            continue;
        }
        as_document = Some(
            serde_json::from_slice::<Value>(
                &resp.0.into_body().into_bytes().await.expect("a body"),
            )
            .expect("the AS metadata is JSON"),
        );
        break;
    }
    let as_document = as_document.unwrap_or_else(|| {
        panic!("no authorization server metadata answered any of {candidates:?}")
    });

    // Every client MUST reject a document naming a different issuer.
    assert_eq!(
        as_document["issuer"], issuer,
        "the AS metadata's issuer must equal the issuer used to build the URL",
    );
    assert!(
        as_document["token_endpoint"].is_string(),
        "the client needs a token endpoint to finish the flow",
    );
}

#[tokio::test]
async fn the_metadata_endpoint_is_reachable_without_a_token() {
    let app = TestApp::for_module::<DiscoveryApp>().await.expect("boots");

    app.http()
        .get(WELL_KNOWN_PATH)
        .send()
        .await
        .assert_status_is_ok();
}

/// `TestApp` is not `Debug`, so `expect_err` cannot take a boot result.
async fn boot_error<M: nest_rs_core::Module + 'static>(msg: &str) -> anyhow::Error {
    match TestApp::for_module::<M>().await {
        Ok(_) => panic!("{msg}"),
        Err(err) => err,
    }
}

#[tokio::test]
async fn a_deployment_that_cannot_name_itself_fails_boot() {
    #[module(imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root(OAuthResourceConfig::default()),
    ])]
    struct Incomplete;

    let err = boot_error::<Incomplete>("an unnamed resource must not boot").await;
    assert!(
        format!("{err:#}").contains(&nest_rs_config::var_name(
            <OAuthResourceConfig as nest_rs_config::Namespaced>::NAMESPACE,
            "RESOURCE",
        )),
        "the boot failure must name the variable: {err:#}",
    );
}

#[tokio::test]
async fn an_unbound_audience_fails_boot() {
    #[module(imports = [
        AuthnModule::for_root(AuthnConfig { secret: Some(SECRET.into()), ..AuthnConfig::default() }),
        OAuthResourceModule::for_root(resource()),
    ])]
    struct Unbound;

    let err = boot_error::<Unbound>("an unbound audience must not boot").await;
    let text = format!("{err:#}");
    assert!(
        text.contains(&nest_rs_config::var_name(
            <AuthnConfig as nest_rs_config::Namespaced>::NAMESPACE,
            "AUDIENCE",
        )),
        "got: {text}",
    );
}

/// RFC 9728 §1.2: a resource identifier uses the https scheme.
#[tokio::test]
async fn a_non_https_resource_identifier_is_refused() {
    #[module(imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some("http://api.example.com".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..OAuthResourceConfig::default()
        }),
    ])]
    struct PlainHttp;

    let err = boot_error::<PlainHttp>("http is not a canonical resource URI").await;
    assert!(
        format!("{err:#}").contains("RFC 9728 §1.2"),
        "the refusal names the clause: {err:#}",
    );
}

/// Loopback is spared, for local development.
#[tokio::test]
async fn http_on_loopback_is_accepted_for_local_development() {
    #[module(imports = [
        AuthnModule::for_root(AuthnConfig {
            secret: Some(SECRET.into()),
            audience: Some("http://localhost:3003".into()),
            ..AuthnConfig::default()
        }),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some("http://localhost:3003".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..OAuthResourceConfig::default()
        }),
    ])]
    struct Loopback;

    TestApp::for_module::<Loopback>()
        .await
        .expect("localhost boots");
}

/// RFC 9728 §2's other defined methods are refused: the framework reads a bearer
/// token only from the `Authorization` header.
#[tokio::test]
async fn a_bearer_method_the_framework_does_not_honour_is_refused() {
    #[module(imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some("https://api.example.com".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            bearer_methods_supported: vec!["body".into()],
            ..OAuthResourceConfig::default()
        }),
    ])]
    struct AdvertisesBody;

    let err = boot_error::<AdvertisesBody>("body is not honoured").await;
    let text = format!("{err:#}");
    assert!(text.contains("Authorization"), "got: {text}");

    #[module(imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some("https://api.example.com".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            bearer_methods_supported: vec!["telepathy".into()],
            ..OAuthResourceConfig::default()
        }),
    ])]
    struct AdvertisesNonsense;

    let err = boot_error::<AdvertisesNonsense>("telepathy is not a defined method").await;
    assert!(format!("{err:#}").contains("RFC 9728 §2"), "got: {err:#}",);
}

/// The route matches the URL path, which never carries the query a `metadata_url`
/// may (RFC 9728 §3.1).
#[tokio::test]
async fn a_resource_with_a_query_serves_the_url_its_challenge_advertises() {
    #[module(imports = [
        AuthnModule::for_root(AuthnConfig {
            secret: Some(SECRET.into()),
            audience: Some("https://api.example.com/mcp?tenant=a".into()),
            ..AuthnConfig::default()
        }),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some("https://api.example.com/mcp?tenant=a".into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..OAuthResourceConfig::default()
        }),
    ])]
    struct QueryResource;

    let app = TestApp::for_module::<QueryResource>().await.expect("boots");

    let response = app
        .http()
        .get(format!("{WELL_KNOWN_PATH}/mcp"))
        .send()
        .await;
    response.assert_status_is_ok();
}
