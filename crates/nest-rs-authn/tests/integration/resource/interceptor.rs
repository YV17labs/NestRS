//! Covers `src/resource/interceptor.rs` across the transports it is meant to
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

use nest_rs_authn::{
    AuthnModule, JwtConfig, ProtectedResourceConfig, ProtectedResourceModule, WELL_KNOWN_PATH,
};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{async_trait, controller, routes};
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

fn resource_server() -> nest_rs_authn::ProtectedResourceSetup {
    ProtectedResourceModule::for_root(
        ProtectedResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"]),
    )
}

/// The step-up tests advertise scopes as well, so the challenge names something
/// the metadata document also carries.
fn scoped_resource_server() -> nest_rs_authn::ProtectedResourceSetup {
    ProtectedResourceModule::for_root(
        ProtectedResourceConfig::default()
            .with_resource(RESOURCE)
            .with_authorization_servers(["https://auth.example.com"])
            .with_scopes_supported(["posts:read", REQUIRED]),
    )
}

// ═══ The 401 — a tokenless client learns where to get one ═══════════════════

// ── MCP ─────────────────────────────────────────────────────────────────────

#[module(imports = [authn(), resource_server()], providers = [EchoTool])]
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

#[injectable]
#[derive(Default)]
struct RefuseUpgrade;

impl Layer for RefuseUpgrade {}

#[async_trait]
impl Guard for RefuseUpgrade {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::unauthorized("missing bearer token"))
    }
}

#[gateway(path = "/ws")]
#[use_guards(RefuseUpgrade)]
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
    imports = [WsModule, authn(), resource_server()],
    providers = [ChatGateway, RefuseUpgrade],
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
