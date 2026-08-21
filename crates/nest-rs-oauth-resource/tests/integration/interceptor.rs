//! Covers `src/interceptor.rs` across the transports it is meant to
//! serve — both refusals the edge decorates.
//!
//! The RFC 9728 challenge is attached at the transport edge precisely so it does
//! not depend on which layer wrote the `401`. These tests boot the real thing on
//! each transport and check that the pointer is there — an MCP client and a WS
//! client discover the authorization server exactly as an HTTP client does.
//!
//! The `401` proves *a client with no token learns where to get one*. The
//! step-up `403` proves the other half: *a client whose token verified but is
//! too narrow learns which scope to ask for* — without it a narrow token meets
//! a bare `403` and the only recovery is guesswork. That denial is raised by an
//! ordinary guard, which is the point: the challenge is written once at the
//! edge, so any guard, extractor or bridge that refuses with
//! [`Denial::insufficient_scope`] gets a conformant answer without knowing this
//! module exists.
//!
//! `/graphql` is absent on purpose: it answers an unauthenticated operation with
//! `200 OK` + an `UNAUTHENTICATED` error frame, so there is no `401` to carry a
//! challenge. Its clients discover through the well-known document, which the
//! spec offers as the equal alternative — `controller.rs` covers that path.

use crate::AlwaysUnauthorized;
use nest_rs_authn::{AuthnModule, JwtConfig};
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

/// The scope the step-up guard below demands, and the one the scoped deployment
/// advertises — so the client is told to ask for something discovery actually
/// names.
const REQUIRED: &str = "posts:write";

/// What the `401` challenge must be, on every transport that can carry one —
/// built from the path the crate publishes at, not a copy of it.
fn expected() -> String {
    format!("Bearer resource_metadata=\"{RESOURCE}{WELL_KNOWN_PATH}\"")
}

fn authn() -> nest_rs_authn::AuthnSetup {
    AuthnModule::for_root(JwtConfig {
        secret: Some(SECRET.into()),
        audience: Some(RESOURCE.into()),
        ..JwtConfig::default()
    })
}

fn discovery() -> nest_rs_oauth_resource::OAuthResourceSetup {
    OAuthResourceModule::for_root(
        OAuthResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"]),
    )
}

/// The step-up tests advertise scopes as well, so the challenge names something
/// the metadata document also carries.
fn scoped_resource_server() -> nest_rs_oauth_resource::OAuthResourceSetup {
    OAuthResourceModule::for_root(
        OAuthResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"])
            .with_scopes_supported(["posts:read", REQUIRED]),
    )
}

// ═══ The 401 — a tokenless client learns where to get one ═══════════════════

// ── MCP ─────────────────────────────────────────────────────────────────────

#[module(imports = [authn(), discovery()], providers = [EchoTool])]
struct McpResourceServer;

#[tokio::test]
async fn an_mcp_endpoint_refusing_an_unauthenticated_call_carries_the_pointer() {
    // `/mcp` is `EdgePosture::Exempt` — it skips the guard chain and denies
    // in-band instead. That bypass must not also bypass discovery: an MCP
    // client's whole flow starts from this response.
    let app = TestApp::for_module::<McpResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&resp.0), expected());
}

// ── WS ──────────────────────────────────────────────────────────────────────

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
    // The upgrade is an HTTP GET carrying the real guards, so its refusal is an
    // ordinary 401 — and a WS client is an OAuth client like any other.
    let app = TestApp::for_module::<WsResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().get("/ws").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&resp.0), expected());
}

// ═══ The step-up 403 — a narrow token learns which scope to ask for ═════════

/// Stands in for the real chain's verdict: the caller authenticated, and the
/// ability layer withheld the rule their token could not reach. What the
/// transport does with that verdict is what these tests are about.
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

/// The refusal that is *not* a scope problem — no wider token fixes it, so no
/// challenge may be emitted.
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

/// Every assertion the step-up challenge has to satisfy, in one place: the
/// error code a client branches on, the scope it must request, and the document
/// it requests it from.
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

// ── HTTP ────────────────────────────────────────────────────────────────────

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
    // Telling this caller to go widen their token sends them somewhere that
    // cannot help — and a client that retries a fresh token forever is a worse
    // outcome than a plain refusal.
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

// ── MCP ─────────────────────────────────────────────────────────────────────

#[module(
    imports = [authn(), scoped_resource_server()],
    providers = [EchoTool, TokenTooNarrow],
)]
struct McpStepUpServer;

#[tokio::test]
async fn an_mcp_scope_denial_carries_the_same_challenge() {
    // `/mcp` is `EdgePosture::Exempt`: it gates in-band, through the global
    // pool folded in by `FallbackMcpGuard`. That path builds its refusal as a
    // poem `Err`, which is exactly where the evidence used to be dropped — so
    // this asserts transport parity *and* the `Err`-path fix underneath it.
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

// ═══ The drift — a scope the deployment never advertises ════════════════════

/// A deployment whose document advertises `posts:read` and nothing else, while
/// the guard above demands `posts:write`. That pairing is a real deployment
/// mistake rather than a contrived one: the guard lives in the feature crate
/// and the document in the environment, so they drift apart one at a time.
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
    // The interceptor is the one place both halves are known, so it is the only
    // place the drift can be caught — and the client cannot see it at all: the
    // challenge it receives is well-formed and names a scope its authorization
    // server will refuse to issue. Nothing but this event stands between that
    // and a client retrying forever.
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<DriftedResourceServer>()
        .await
        .expect("boots");

    let resp = app.http().post("/posts").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    // The challenge is still emitted: the drift is the deployment's to fix, and
    // withholding the pointer would help no one.
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

// ═══ A scope name that cannot go in a header ════════════════════════════════
//
// The resource URI and the metadata URL are character-checked at boot, so the
// only way a challenge becomes unrepresentable is a **scope** — and scopes do
// not come from config. `Denial::insufficient_scope([..])` takes whatever the
// guard hands it, in application code the config never sees.
//
// What must not happen is a 403 that silently loses its `WWW-Authenticate`: the
// client is then told "forbidden" with no code and no pointer, which reads as a
// final refusal, and the step-up never happens. Nothing about the response says
// the header was dropped, so the event is the only trace.

/// A scope carrying a newline — RFC 6749 forbids it in a scope token, and
/// `HeaderValue::from_str` refuses the challenge built around it.
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
    // The enriched challenge cannot be built, so what stands is the one the
    // guard layer wrote from a *static* RFC 6750 §3.1 code. The safety property
    // is unchanged and asserted below: the offending scope never reaches the
    // wire — the alternative would be smuggling a newline into a response
    // header. What changed is that a caller now reads why they were refused
    // instead of a bare `403`, which is also what an app that mounts no
    // discovery document gets.
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

/// A guard that refuses the way a real authentication guard does: an
/// `AuthError` rendered through its own `IntoResponse`, so the response carries
/// the RFC 6750 §3.1 `error` code this layer cannot reconstruct.
#[injectable]
struct ExpiredCredential;

impl Layer for ExpiredCredential {}

#[async_trait]
impl Guard for ExpiredCredential {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        // Exactly what `AuthnGuard` returns for an expired token: the RFC 6750
        // §3.1 code travels on the denial, and `denial_to_http_response` writes
        // the challenge naming it.
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

/// The merge, and the regression it closes. A handler's own challenge carries
/// the RFC 6750 §3.1 `error` code — which the interceptor cannot reconstruct,
/// because only the authentication layer knows *why* the credential failed —
/// so the discovery pointer is spliced in beside it rather than over it.
///
/// Keying on "is it exactly the bare word `Bearer`" made this case silently
/// lose the pointer the moment the framework started emitting a conformant
/// challenge: the response was no longer plain, so the interceptor skipped it.
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
