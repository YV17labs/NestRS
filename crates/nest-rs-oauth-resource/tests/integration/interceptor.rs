//! Covers `src/interceptor.rs` across the transports it is meant to
//! serve — both refusals the edge decorates.
//!
//! `/graphql` is absent: it answers an unauthenticated operation `200 OK` with an
//! `UNAUTHENTICATED` frame, so there is no `401` to carry a challenge.

use crate::AlwaysUnauthorized;
use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::challenge::{BEARER, NoBearerChallenge};
use nest_rs_http::{StatusCode, header};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule, WELL_KNOWN_PATH};
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsModule, gateway, messages};

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
    OAuthResourceModule::for_root(OAuthResourceConfig {
        resource: Some(RESOURCE.into()),
        authorization_servers: vec!["https://auth.example.com".into()],
        ..Default::default()
    })
}

fn scoped_resource_server() -> nest_rs_oauth_resource::OAuthResourceSetup {
    OAuthResourceModule::for_root(OAuthResourceConfig {
        resource: Some(RESOURCE.into()),
        authorization_servers: vec!["https://auth.example.com".into()],
        scopes_supported: vec!["posts:read".into(), REQUIRED.into()],
        ..Default::default()
    })
}

#[module(imports = [authn(), discovery()], providers = [EchoTool])]
struct McpResourceServer;

#[tokio::test]
async fn an_mcp_endpoint_refusing_an_unauthenticated_call_carries_the_pointer() {
    // `/mcp` refuses at its own request stage; discovery still decorates the refusal.
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
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::insufficient_scope([REQUIRED], "forbidden"))
    }
}

impl GuardFor<nest_rs_http::HttpView> for TokenTooNarrow {}

/// A refusal no wider token fixes, so no challenge may be emitted.
#[injectable]
#[derive(Default)]
struct NeverAllowed;

impl Layer for NeverAllowed {}

#[async_trait]
impl Guard for NeverAllowed {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("forbidden"))
    }
}

impl GuardFor<nest_rs_http::HttpView> for NeverAllowed {}

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
    // `/mcp` is a self-mount: the transport's request stage runs the global pool.
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
    OAuthResourceModule::for_root(OAuthResourceConfig {
        resource: Some(RESOURCE.into()),
        authorization_servers: vec!["https://auth.example.com".into()],
        scopes_supported: vec!["posts:read".into()],
        ..Default::default()
    })
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

// A scope outside RFC 6749 §3.3's grammar: scopes come from a guard, not the
// config checked at boot, so the challenge leaves the attribute out and says so.

/// A scope carrying a space, which would read as two scopes in the attribute.
const OUTSIDE_THE_GRAMMAR: &str = "posts write";

/// What the operator reads when the challenge goes without its scopes.
const LEFT_OUT: &str =
    "a required scope is not a scope-token; the challenge is sent without its scope attribute";

#[injectable]
#[derive(Default)]
struct ScopeOutsideTheGrammar;

impl Layer for ScopeOutsideTheGrammar {}

#[async_trait]
impl Guard for ScopeOutsideTheGrammar {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::insufficient_scope(
            [OUTSIDE_THE_GRAMMAR],
            "forbidden",
        ))
    }
}

impl GuardFor<nest_rs_http::HttpView> for ScopeOutsideTheGrammar {}

#[controller(path = "/malformed")]
#[use_guards(ScopeOutsideTheGrammar)]
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
    providers = [MalformedScopeController, ScopeOutsideTheGrammar],
)]
struct MalformedScopeServer;

#[tokio::test]
async fn a_scope_outside_the_grammar_leaves_the_attribute_out_and_is_reported() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<MalformedScopeServer>()
        .await
        .expect("boots — the scope is the guard's, not the config's");

    let resp = app.http().get("/malformed").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        challenge(&resp.0),
        format!(
            "Bearer error=\"insufficient_scope\", resource_metadata=\"{RESOURCE}{WELL_KNOWN_PATH}\""
        ),
        "the step-up challenge still points at the document, without the scope it cannot carry",
    );

    let said: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| event.message == LEFT_OUT)
        .collect();
    let [event] = said.as_slice() else {
        panic!("one refusal files one line: {said:#?}");
    };
    assert_eq!(
        (event.target.as_str(), event.level.as_str()),
        (nest_rs_http::target::HTTP, "warn"),
        "the edge stamped the guard's step-up and said it",
    );
    assert!(
        event
            .field("scopes")
            .is_some_and(|scopes| scopes.contains(OUTSIDE_THE_GRAMMAR)),
        "the line names the rule's scopes, got {:?}",
        event.fields,
    );
    assert!(
        logs.events()
            .iter()
            .filter(|event| event.target == nest_rs_oauth_resource::TARGET)
            .all(|event| event.field("challenge").is_none()),
        "no line carries the challenge, whose pointer holds the resource's URI: {:#?}",
        logs.events(),
    );
}

/// A guard that refuses the way a real authentication guard does, so the response
/// carries the RFC 6750 §3.1 `error` code this layer cannot reconstruct.
#[injectable]
struct ExpiredCredential;

impl Layer for ExpiredCredential {}

#[async_trait]
impl Guard for ExpiredCredential {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        // What `AuthnGuard` returns for an expired token.
        Err(Denial::invalid_credential("invalid token", "invalid_token"))
    }
}

impl GuardFor<nest_rs_http::HttpView> for ExpiredCredential {}

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
    response.assert_status(nest_rs_http::StatusCode::UNAUTHORIZED);

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

/// A login refusing a password through `?` on `CredentialError`, whose `401`
/// says *these credentials are wrong*, and a bare `401` beside it.
#[controller(path = "/session")]
struct SessionController;

#[routes]
impl SessionController {
    #[post("/")]
    #[public]
    async fn login(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_authn::CredentialError.into())
    }

    #[get("/expired")]
    #[public]
    async fn expired(&self) -> nest_rs_http::Result<&'static str> {
        Err(StatusCode::UNAUTHORIZED.into())
    }
}

#[module(imports = [authn(), discovery()], providers = [SessionController])]
struct SessionServer;

/// The marker a handler's `Err` carries out of its rendering reaches the unit
/// stage and the client's response, so a wrong password is answered the bare
/// challenge every `401` owes and never the resource's pointer, while a bare
/// `401` still gains it.
#[tokio::test]
async fn no_bearer_challenge_survives_rendering() {
    let app = TestApp::for_module::<SessionServer>().await.expect("boots");

    let refused = app.http().post("/session").send().await;
    refused.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenge(&refused.0),
        BEARER,
        "a wrong password points at no authorization server",
    );
    assert!(
        refused.0.extensions().get::<NoBearerChallenge>().is_some(),
        "the mark the handler's error set reaches the response the client is sent",
    );

    let bare = app.http().get("/session/expired").send().await;
    bare.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(challenge(&bare.0), expected());
}
