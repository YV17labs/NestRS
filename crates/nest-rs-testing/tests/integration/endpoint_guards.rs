//! `EndpointGuards` as a portable feature declares them: a module adds guards
//! to the request of the in-band endpoint whose units it serves, in an app with
//! no global pool, each guard running once however many modules declare it,
//! and a module the app does not import adding nothing. A browser origin the
//! transport refuses pays for no guard.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, Posture, UnitContext, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{EndpointGuards, HttpView, StatusCode, async_trait};
use nest_rs_mcp::rmcp;
use nest_rs_mcp::{McpView, ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{initialize_request, post_message, post_message_with};
use nest_rs_ws::{WsModule, gateway, messages};

/// How many requests a [`Bearer`] checked.
#[injectable]
#[derive(Default)]
struct Checks {
    seen: AtomicUsize,
}

impl Checks {
    fn take(&self) -> usize {
        self.seen.swap(0, Ordering::SeqCst)
    }
}

/// Stands in for `AuthnGuard` on a gated endpoint: requires a bearer, counting
/// each request it checks.
#[injectable]
struct Bearer {
    #[inject]
    checks: Arc<Checks>,
}

impl Layer for Bearer {}

#[async_trait]
impl Guard for Bearer {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        self.checks.seen.fetch_add(1, Ordering::SeqCst);
        if cx.reflector().posture() == Posture::InBand
            || HttpView::of(cx)?
                .request()
                .header("authorization")
                .is_some()
        {
            return Ok(());
        }
        Err(Denial::unauthorized("missing bearer token"))
    }
}

impl GuardFor<HttpView> for Bearer {}

#[mcp]
#[derive(Clone)]
struct AssistantTool;

#[tool_router(allow_empty)]
impl AssistantTool {}

#[tool_handler]
impl ServerHandler for AssistantTool {}

/// A portable feature: its `/mcp` tools, and the guards their endpoint's
/// request needs, whatever app imports it.
#[module(providers = [Checks, Bearer, AssistantTool, EndpointGuards<McpView, (Bearer,)>])]
struct AssistantFeature;

/// A second feature declaring the same guard for the same endpoint.
#[module(imports = [AssistantFeature], providers = [EndpointGuards<McpView, (Bearer,)>])]
struct SecondFeature;

/// The assistant shape: no global pool, the feature's declaration alone.
#[module(imports = [AssistantFeature])]
struct AssistantApp;

#[module(imports = [AssistantFeature, SecondFeature])]
struct TwoFeaturesApp;

async fn initialize(app: &TestApp, bearer: Option<&str>) -> StatusCode {
    post_message(app.http(), "/mcp", None, bearer, &initialize_request())
        .await
        .0
        .status()
}

#[tokio::test]
async fn a_features_endpoint_guards_authenticate_mcp_in_an_app_with_no_global_pool() {
    let app = TestApp::for_module::<AssistantApp>().await.expect("boots");

    assert_eq!(initialize(&app, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(initialize(&app, Some("Bearer t")).await, StatusCode::OK);
}

#[tokio::test]
async fn two_features_declaring_one_guard_run_it_once_per_request() {
    let app = TestApp::for_module::<TwoFeaturesApp>()
        .await
        .expect("boots");
    let checks = app.container().get::<Checks>().expect("the counter");

    assert_eq!(initialize(&app, Some("Bearer t")).await, StatusCode::OK);
    assert_eq!(
        checks.take(),
        1,
        "one guard, one check, however many declare it"
    );
}

/// The feature's tools without its declaration: what declares the guards is
/// the module the app imports, never the linked crate.
#[module(providers = [AssistantTool])]
struct ToolsOnly;

#[tokio::test]
async fn a_module_the_app_does_not_import_contributes_nothing() {
    let app = TestApp::for_module::<ToolsOnly>().await.expect("boots");

    assert_eq!(
        initialize(&app, Some("Bearer t")).await,
        StatusCode::UNAUTHORIZED,
        "no guard at the request: the gated endpoint denies every request",
    );
}

#[resolver]
struct OriginResolver;

#[operations]
impl OriginResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[gateway(path = "/ws/origin")]
#[derive(Default)]
struct OriginGateway;

#[messages]
impl OriginGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(
    imports = [GraphqlModule::for_root(None), WsModule],
    providers = [Checks, Bearer, AssistantTool, OriginResolver, OriginGateway],
)]
struct EveryUpgradeModule;

/// A WebSocket upgrade under `headers`, a foreign browser origin among them.
async fn upgrade(app: &TestApp, path: &str, protocol: Option<&str>) -> StatusCode {
    let mut request = app
        .http()
        .get(path)
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("host", "api.example")
        .header("origin", "https://evil.example");
    if let Some(protocol) = protocol {
        request = request.header("sec-websocket-protocol", protocol);
    }
    request.send().await.0.status()
}

/// The origin check runs ahead of the request's guards on every surface that
/// owes one — a gateway's upgrade, a graphql-ws upgrade, every `/mcp` request.
#[tokio::test]
async fn a_refused_origin_pays_for_no_guard() {
    let app = TestApp::builder()
        .module::<EveryUpgradeModule>()
        .use_guards_global([guard::<Bearer>()])
        .build()
        .await
        .expect("boots");
    let checks = app.container().get::<Checks>().expect("the counter");

    assert_eq!(
        upgrade(&app, "/ws/origin", None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upgrade(&app, "/graphql", Some("graphql-transport-ws")).await,
        StatusCode::FORBIDDEN,
    );
    post_message_with(
        app.http(),
        "/mcp",
        None,
        Some("Bearer t"),
        &[("host", "api.example"), ("origin", "https://evil.example")],
        &initialize_request(),
    )
    .await
    .assert_status(StatusCode::FORBIDDEN);
    assert_eq!(checks.take(), 0, "no guard ran for a refused origin");
}
