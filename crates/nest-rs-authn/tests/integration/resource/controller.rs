//! Covers `src/resource/controller.rs`: the RFC 9728 discovery flow it serves,
//! walked end to end by a client that knows only one thing: the URL of a
//! protected route.
//!
//! This is the conformance test the capability exists for. It does not assert
//! that a header is present — it *uses* the header the way an MCP client does,
//! and every hop is a real request through the booted app:
//!
//! ```text
//! GET /posts            → 401 + WWW-Authenticate: Bearer resource_metadata="…"
//! GET <resource_metadata>  → the RFC 9728 document
//! GET <authorization_servers[0]>/.well-known/oauth-authorization-server
//!                       → the AS metadata, whose `issuer` the client validates
//! ```
//!
//! Break any link — drop the challenge, serve the document at the wrong path,
//! advertise an issuer nobody serves — and the walk stops at that hop.

use nest_rs_authn::{
    AuthnModule, JwtConfig, ProtectedResourceConfig, ProtectedResourceModule, WELL_KNOWN_PATH,
};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::Request;
use poem::http::{StatusCode, header};
use serde_json::Value;

/// This deployment's canonical URI. `TestClient` speaks paths, so the origin is
/// the loopback one a client would see locally.
const RESOURCE: &str = "http://localhost";
/// The authorization server this resource advertises — served, in this test, by
/// the stub controller below so the last hop is a real response too. It carries
/// a path component on purpose: that is the case where a client must walk the
/// RFC 8414 §3.1 priority order rather than guess one URL.
const ISSUER: &str = "http://localhost/stub-as";
/// 32 bytes: HS256's floor, enforced in `JwtService::new`.
const SECRET: &str = "discovery-flow-secret-0123456789";
/// The one scope this deployment advertises — the challenge is asserted against
/// the same constant the config names, so the two cannot drift apart.
const SCOPE: &str = "posts:read";

// --- the protected resource ------------------------------------------------

/// Refuses every caller, which is what an unauthenticated MCP or HTTP client
/// meets. It denies through `Denial::unauthorized`, the framework's ordinary
/// guard path — the 401 that carries `problem+json` and, before this
/// capability, no challenge whatsoever.
#[injectable]
#[derive(Default)]
struct AlwaysUnauthorized;

impl Layer for AlwaysUnauthorized {}

#[async_trait]
impl Guard for AlwaysUnauthorized {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::unauthorized("missing bearer token"))
    }
}

impl HttpGuard for AlwaysUnauthorized {}

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

// --- the stub authorization server -----------------------------------------

/// Stands in for the authorization server's own metadata endpoint. It exists so
/// the final hop of the flow is an actual request whose `issuer` the client
/// validates, rather than an assertion that a string was present in the previous
/// document.
///
/// It answers only the **third** endpoint in the discovery priority order
/// (OpenID Connect path-appending), so the client has to fall through the first
/// two — which is the behaviour the spec requires and the reason a client that
/// probes a single URL is not conformant.
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

// --- the app ---------------------------------------------------------------

fn jwt() -> JwtConfig {
    JwtConfig {
        secret: Some(SECRET.into()),
        // The audience `ProtectedResourceModule` requires — and requires to be
        // the resource identifier, so a token minted for another service is
        // rejected rather than replayed here.
        audience: Some(RESOURCE.into()),
        ..JwtConfig::default()
    }
}

fn resource() -> ProtectedResourceConfig {
    ProtectedResourceConfig::default()
        .with_resource(RESOURCE)
        .with_authorization_servers([ISSUER])
        .with_scopes_supported([SCOPE])
}

#[module(
    imports = [
        AuthnModule::for_root(jwt()),
        ProtectedResourceModule::for_root(resource()),
    ],
    providers = [PostsController, StubAuthorizationServer, AlwaysUnauthorized],
)]
struct DiscoveryApp;

/// Pull a quoted parameter out of a `WWW-Authenticate` value, the way a client
/// parses the challenge it was handed.
fn challenge_param(challenge: &str, name: &str) -> Option<String> {
    let start = challenge.find(&format!("{name}=\""))? + name.len() + 2;
    let rest = &challenge[start..];
    Some(rest[..rest.find('"')?].to_owned())
}

/// `TestClient` addresses paths; a client on a socket would use the absolute
/// URL. Asserting the prefix before stripping it is what keeps this honest.
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
    let challenge = denied
        .0
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("a 401 from a protected resource must carry a challenge")
        .to_str()
        .expect("ascii")
        .to_owned();

    // The challenge also tells the client which scopes to ask for, so it does
    // not request more than the operation needs.
    assert_eq!(challenge_param(&challenge, "scope").as_deref(), Some(SCOPE));

    // Hop 2 — follow `resource_metadata` rather than guessing the well-known
    // path. That the two agree is the property under test.
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

    // Hop 3 — the issuer has a path component, so a conformant client tries the
    // three well-known endpoints in the order the spec fixes and takes the
    // first that answers.
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

    // The validation every client MUST perform: a document served from an
    // issuer's well-known URL that names a *different* issuer is rejected.
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
    // Declared `#[public]`, not public by omission. A discovery endpoint behind
    // the guard chain would be a flow that can never start: the client has no
    // token precisely because it has not read this document yet.
    let app = TestApp::for_module::<DiscoveryApp>().await.expect("boots");

    app.http()
        .get(WELL_KNOWN_PATH)
        .send()
        .await
        .assert_status_is_ok();
}

/// `TestApp` is not `Debug`, so `expect_err` cannot be used directly on a boot
/// result; this keeps the failure message at the call site.
async fn boot_error<M: nest_rs_core::Module + 'static>(msg: &str) -> anyhow::Error {
    match TestApp::for_module::<M>().await {
        Ok(_) => panic!("{msg}"),
        Err(err) => err,
    }
}

#[tokio::test]
async fn a_deployment_that_cannot_name_itself_fails_boot() {
    // The other half of the contract: the capability refuses to advertise a
    // resource identity it has not been given, rather than serving a document
    // that tells a client nothing.
    #[module(imports = [
        AuthnModule::for_root(jwt()),
        ProtectedResourceModule::for_root(ProtectedResourceConfig::default()),
    ])]
    struct Incomplete;

    let err = boot_error::<Incomplete>("an unnamed resource must not boot").await;
    assert!(
        format!("{err:#}").contains("NESTRS_AUTHN__RESOURCE"),
        "the boot failure must name the variable: {err:#}",
    );
}

#[tokio::test]
async fn an_unbound_audience_fails_boot() {
    // The confused-deputy defence. Without `aud` pinned, this server accepts
    // any token its issuer signed — including one a user granted to a different
    // service, which that service can replay here.
    #[module(imports = [
        AuthnModule::for_root(JwtConfig { secret: Some(SECRET.into()), ..JwtConfig::default() }),
        ProtectedResourceModule::for_root(resource()),
    ])]
    struct Unbound;

    let err = boot_error::<Unbound>("an unbound audience must not boot").await;
    let text = format!("{err:#}");
    assert!(text.contains("NESTRS_AUTHN__AUDIENCE"), "got: {text}");
}
