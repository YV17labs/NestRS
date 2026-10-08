//! Covers `src/interceptor.rs` across the transports it is meant to
//! serve — both refusals the edge decorates.
//!
//! `/graphql` is absent: it answers an unauthenticated operation `200 OK` with an
//! `UNAUTHENTICATED` frame, so there is no `401` to carry a challenge.

use crate::AlwaysUnauthorized;
use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule, WELL_KNOWN_PATH};
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsModule, gateway, messages};
use poem::Request;
use poem::http::{StatusCode, header};

use crate::{EchoTool, challenge};

const RESOURCE: &str = "http://localhost";
const SECRET: &str = "transport-parity-secret-0123456789";

/// The scope the step-up guard below demands, and the one the scoped deployment advertises.
const REQUIRED: &str = "posts:write";

/// What the `401` challenge must be, on every transport that can carry one.
fn expected() -> String {
    format!("Bearer resource_metadata=\"{RESOURCE}{WELL_KNOWN_PATH}\"")
}

fn authn() -> nest_rs_authn::AuthnSetup {
    AuthnModule::for_root(AuthnConfig {
        secret: Some(SECRET.into()),
        audience: Some(RESOURCE.into()),
        ..AuthnConfig::default()
    })
}

fn discovery() -> nest_rs_oauth_resource::OAuthResourceSetup {
    OAuthResourceModule::for_root(
        OAuthResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"]),
    )
}

fn scoped_resource_server() -> nest_rs_oauth_resource::OAuthResourceSetup {
    OAuthResourceModule::for_root(
        OAuthResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"])
            .with_scopes_supported(["posts:read", REQUIRED]),
    )
}

#[module(imports = [authn(), discovery()], providers = [EchoTool])]
struct McpResourceServer;

#[tokio::test]
async fn an_mcp_endpoint_refusing_an_unauthenticated_call_carries_the_pointer() {
    // `/mcp` is `EdgePosture::Exempt`: it skips the guard chain, not discovery.
    let app = TestApp::for_module::<McpResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&resp.0), expected());
}

#[gateway(path = "/ws")]
#[use_guards(AlwaysUnauthorized)]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(
    imports = [WsModule, authn(), discovery()],
    providers = [ChatGateway, AlwaysUnauthorized],
)]
struct WsResourceServer;

#[tokio::test]
async fn a_refused_websocket_upgrade_carries_the_pointer() {
    let app = TestApp::for_module::<WsResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().get("/ws").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&resp.0), expected());
}

/// Stands in for the real chain's verdict: authenticated, but the token's scope falls short.
#[injectable]
#[derive(Default)]
struct TokenTooNarrow;

impl Layer for TokenTooNarrow {}

#[async_trait]
impl Guard for TokenTooNarrow {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::insufficient_scope([REQUIRED], "forbidden"))
    }
}

impl HttpGuard for TokenTooNarrow {}

/// A refusal no wider token fixes, so no challenge may be emitted.
#[injectable]
#[derive(Default)]
struct NeverAllowed;

impl Layer for NeverAllowed {}

#[async_trait]
impl Guard for NeverAllowed {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::forbidden("forbidden"))
    }
}

impl HttpGuard for NeverAllowed {}

/// The step-up challenge: its error code, the scope to request, and the document.
fn assert_is_step_up(challenge: &str) {
    assert!(
        challenge.contains("error=\"insufficient_scope\""),
        "RFC 6750 §3.1 — without the code a client cannot tell this from a final refusal: {challenge}",
    );
    assert!(
        challenge.contains(&format!("scope=\"{REQUIRED}\"")),
        "the client is told exactly what to ask for: {challenge}",
    );
    assert!(
        challenge.contains(&format!(
            "resource_metadata=\"{RESOURCE}{WELL_KNOWN_PATH}\""
        )),
        "and where to ask — the same document the 401 points at: {challenge}",
    );
}

#[controller(path = "/posts")]
#[use_guards(TokenTooNarrow)]
struct PostsController;

#[routes]
impl PostsController {
    #[post("/")]
    async fn create(&self) -> &'static str {
        "created"
    }
}

#[controller(path = "/admin")]
#[use_guards(NeverAllowed)]
struct AdminController;

#[routes]
impl AdminController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "admin"
    }
}

#[module(
    imports = [authn(), scoped_resource_server()],
    providers = [PostsController, TokenTooNarrow, AdminController, NeverAllowed],
)]
struct HttpResourceServer;

#[tokio::test]
async fn an_http_scope_denial_tells_the_client_which_scope_to_request() {
    let app = TestApp::for_module::<HttpResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().post("/posts").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    assert_is_step_up(&challenge(&resp.0));
}

#[tokio::test]
async fn an_ordinary_forbidden_carries_no_challenge_at_all() {
    let app = TestApp::for_module::<HttpResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().get("/admin").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    assert!(
        resp.0.headers().get(header::WWW_AUTHENTICATE).is_none(),
        "a final refusal must not advertise a recovery that does not exist",
    );
}

#[module(
    imports = [authn(), scoped_resource_server()],
    providers = [EchoTool, TokenTooNarrow],
)]
struct McpStepUpServer;

#[tokio::test]
async fn an_mcp_scope_denial_carries_the_same_challenge() {
    // `/mcp` gates in-band, through `FallbackMcpGuard`, whose refusal is a poem `Err`.
    let app = TestApp::builder()
        .module::<McpStepUpServer>()
        .use_guards_global([guard::<TokenTooNarrow>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    assert_is_step_up(&challenge(&resp.0));
}

/// A deployment advertising `posts:read` alone while the guard demands `posts:write`.
fn drifted_resource_server() -> nest_rs_oauth_resource::OAuthResourceSetup {
    OAuthResourceModule::for_root(
        OAuthResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"])
            .with_scopes_supported(["posts:read"]),
    )
}

#[module(
    imports = [authn(), drifted_resource_server()],
    providers = [PostsController, TokenTooNarrow],
)]
struct DriftedResourceServer;

#[tokio::test]
async fn a_scope_the_document_never_advertises_is_reported_at_warn() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<DriftedResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().post("/posts").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    assert_is_step_up(&challenge(&resp.0));

    let event = logs.expect_one(
        nest_rs_oauth_resource::TARGET,
        "denied for a scope this resource does not advertise — a client following \
         the metadata document cannot request it",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("scope_not_advertised")
    );
    assert!(
        event.field("scopes").is_some_and(|s| s.contains(REQUIRED)),
        "the event names the scope that is missing from the document, got {:?}",
        event.fields,
    );
}

// A scope name that cannot go in a header: scopes come from a guard, not the config
// checked at boot, and a dropped `WWW-Authenticate` leaves only the event as a trace.

/// A scope carrying a newline, which `HeaderValue::from_str` refuses.
const UNREPRESENTABLE: &str = "posts:\nwrite";

#[injectable]
#[derive(Default)]
struct ScopeWithAControlCharacter;

impl Layer for ScopeWithAControlCharacter {}

#[async_trait]
impl Guard for ScopeWithAControlCharacter {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::insufficient_scope([UNREPRESENTABLE], "forbidden"))
    }
}

impl HttpGuard for ScopeWithAControlCharacter {}

#[controller(path = "/malformed")]
#[use_guards(ScopeWithAControlCharacter)]
struct MalformedScopeController;

#[routes]
impl MalformedScopeController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "never reached"
    }
}

#[module(
    imports = [authn(), scoped_resource_server()],
    providers = [MalformedScopeController, ScopeWithAControlCharacter],
)]
struct MalformedScopeServer;

#[tokio::test]
async fn a_scope_that_cannot_be_a_header_value_is_reported_rather_than_dropped() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<MalformedScopeServer>()
        .await
        .expect("boots — the scope is the guard's, not the config's");

    let resp = app.http().get("/malformed").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    // The challenge the guard layer wrote from a static RFC 6750 §3.1 code stands;
    // the offending scope never reaches the wire.
    let challenge = resp
        .0
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("the static code stands even when the scope list cannot be sent")
        .to_str()
        .expect("a challenge built from constants is a valid header value");
    assert_eq!(challenge, r#"Bearer error="insufficient_scope""#);
    assert!(
        !challenge.contains("posts:"),
        "the scope that could not be encoded is not on the wire, got {challenge:?}",
    );

    let event = logs.expect_one(
        nest_rs_oauth_resource::TARGET,
        "insufficient-scope challenge is not a valid header value",
    );
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("challenge")
            .is_some_and(|c| c.contains("posts:")),
        "the event carries the challenge that could not be sent, which is what \
         points at the offending scope, got {:?}",
        event.fields,
    );
    assert!(
        event.field("error").is_some(),
        "…and why it could not, got {:?}",
        event.fields,
    );
}

/// A guard that refuses the way a real authentication guard does, so the response
/// carries the RFC 6750 §3.1 `error` code this layer cannot reconstruct.
#[injectable]
struct ExpiredCredential;

impl Layer for ExpiredCredential {}

#[async_trait]
impl Guard for ExpiredCredential {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        // What `AuthnGuard` returns for an expired token.
        Err(Denial::invalid_credential("invalid token", "invalid_token"))
    }
}

impl HttpGuard for ExpiredCredential {}

#[controller(path = "/guarded")]
#[use_guards(ExpiredCredential)]
struct GuardedController;

#[routes]
impl GuardedController {
    #[get("/")]
    #[public]
    async fn index(&self) -> &'static str {
        "unreachable"
    }
}

#[module(
    imports = [authn(), discovery()],
    providers = [GuardedController, ExpiredCredential],
)]
struct ChallengeApp;

/// A handler's own challenge carries the RFC 6750 §3.1 `error` code, so the
/// discovery pointer is spliced in beside it rather than over it.
#[tokio::test]
async fn a_challenge_carrying_an_error_code_keeps_it_and_gains_the_pointer() {
    let app = TestApp::for_module::<ChallengeApp>().await.expect("boots");

    let response = app
        .http()
        .get("/guarded")
        .header("authorization", "Bearer expired-token")
        .send()
        .await;
    response.assert_status(poem::http::StatusCode::UNAUTHORIZED);

    let challenge = crate::challenge(&response.0);

    assert!(
        challenge.contains(r#"error="invalid_token""#),
        "the handler's reason survives: {challenge}",
    );
    assert!(
        challenge.contains("resource_metadata="),
        "and the deployment's pointer is merged in: {challenge}",
    );
}
