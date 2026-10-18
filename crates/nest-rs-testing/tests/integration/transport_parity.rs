//! `/graphql` and `/mcp` answer the **same app wiring** the same way, the
//! transport running the guards at their request under three wirings: a global
//! pool alone, guards a module declares for the endpoint (`EndpointGuards`), and
//! neither. The pool always runs at the request — `/graphql`'s POST and its
//! graphql-ws upgrade alike; the endpoint's guards add to it, and a guard listed
//! in both runs once.
//!
//! The deliberate difference: `/graphql` carries `Posture::InBand`, so a pooled
//! authentication guard admits an anonymous operation to the resolver gates;
//! `/mcp` carries none (its requests read `Gated`), and refuses.

use std::sync::{Arc, Mutex};

use nest_rs_core::{Layer, Posture, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{EndpointGuards, HttpView, async_trait};
use nest_rs_mcp::{McpView, ServerHandler, mcp, tool_handler, tool_router};
// The name rmcp's `#[tool_router]` / `#[tool_handler]` expansions resolve.
use nest_rs_http::StatusCode;
use nest_rs_mcp::rmcp;
use nest_rs_testing::mcp::{initialize_request, post_message, post_message_with};
use nest_rs_testing::{TestApp, TestResponse};

/// Stands in for `AuthnGuard`: requires a bearer unless the surface admits an
/// anonymous caller — a public handler, or an in-band endpoint whose
/// operations decide.
#[injectable]
#[derive(Default)]
struct BearerGuard;

impl Layer for BearerGuard {}

#[async_trait]
impl Guard for BearerGuard {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        if matches!(cx.reflector().posture(), Posture::Public | Posture::InBand)
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

impl GuardFor<HttpView> for BearerGuard {}

/// An IP allow-list or an API key: a check on the HTTP request alone.
#[injectable]
#[derive(Default)]
struct ApiKeyGuard;

impl Layer for ApiKeyGuard {}

#[async_trait]
impl Guard for ApiKeyGuard {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        if HttpView::of(cx)?.request().header(API_KEY).is_some() {
            return Ok(());
        }
        Err(Denial::forbidden("an API key is required"))
    }
}

const API_KEY: &str = "x-api-key";

/// The documented idiom of a guard admitting a `#[public]` handler, and no one
/// else.
#[injectable]
#[derive(Default)]
struct PublicOnlyGuard;

impl Layer for PublicOnlyGuard {}

#[async_trait]
impl Guard for PublicOnlyGuard {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        if cx.reflector().is_public() {
            return Ok(());
        }
        Err(Denial::unauthorized("only public handlers"))
    }
}

/// The postures [`PostureProbe`] read, in order.
#[injectable]
#[derive(Default)]
struct Postures {
    seen: Mutex<Vec<Posture>>,
}

impl Postures {
    fn take(&self) -> Vec<Posture> {
        std::mem::take(&mut *self.seen.lock().expect("postures lock"))
    }
}

/// Admits every request after writing down the posture it read.
#[injectable]
struct PostureProbe {
    #[inject]
    postures: Arc<Postures>,
}

impl Layer for PostureProbe {}

#[async_trait]
impl Guard for PostureProbe {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let posture = cx.reflector().posture();
        self.postures
            .seen
            .lock()
            .expect("postures lock")
            .push(posture);
        Ok(())
    }
}

/// Admits everything: the authorization half of endpoint guards whose
/// assertions are about their authentication half.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

impl Guard for PassGuard {}

impl GuardFor<HttpView> for PassGuard {}

/// The `check_http` calls of the recording guards below, in the order they ran.
#[injectable]
#[derive(Default)]
struct Trail {
    seen: Mutex<Vec<&'static str>>,
}

impl Trail {
    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.seen.lock().expect("trail lock"))
    }
}

/// A guard admitting every request after writing its name on the [`Trail`].
macro_rules! recording_guard {
    ($name:ident, $label:literal) => {
        #[injectable]
        struct $name {
            #[inject]
            trail: Arc<Trail>,
        }

        impl Layer for $name {}

        #[async_trait]
        impl Guard for $name {
            async fn check_http(
                &self,
                _cx: &mut nest_rs_core::UnitContext<'_>,
            ) -> Result<(), Denial> {
                self.trail.seen.lock().expect("trail lock").push($label);
                Ok(())
            }
        }

        impl GuardFor<HttpView> for $name {}
    };
}

recording_guard!(FirstGuard, "first");
recording_guard!(CoveredGuard, "covered");
recording_guard!(SecondGuard, "second");
recording_guard!(LastGuard, "last");

#[resolver]
struct ParityResolver;

#[operations]
impl ParityResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[mcp]
#[derive(Clone)]
struct ParityTool;

#[tool_router(allow_empty)]
impl ParityTool {}

#[tool_handler]
impl ServerHandler for ParityTool {}

/// One module, both transports, **no** endpoint guards on either side.
#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        BearerGuard,
        PublicOnlyGuard,
        Postures,
        PostureProbe,
        ParityResolver,
        ParityTool,
    ],
)]
struct BothTransportsModule;

/// Same, plus guards declared for `/mcp` that admit every request.
#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        BearerGuard,
        PassGuard,
        ParityResolver,
        ParityTool,
        EndpointGuards<McpView, (PassGuard,)>,
    ],
)]
struct DeclaredMcpModule;

/// Both endpoints' guards authenticating a bearer, beside an HTTP-only guard
/// the app lists in its pool.
#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        ApiKeyGuard,
        BearerGuard,
        PassGuard,
        Postures,
        PostureProbe,
        ParityResolver,
        ParityTool,
        EndpointGuards<GraphqlView, (BearerGuard, PassGuard)>,
        EndpointGuards<McpView, (BearerGuard, PassGuard)>,
    ],
)]
struct BearerDeclaredModule;

/// Both endpoints' guards over recording guards, the pool chosen per test.
#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        Trail,
        FirstGuard,
        CoveredGuard,
        SecondGuard,
        LastGuard,
        ParityResolver,
        ParityTool,
        EndpointGuards<GraphqlView, (CoveredGuard, SecondGuard)>,
        EndpointGuards<McpView, (CoveredGuard, SecondGuard)>,
    ],
)]
struct RecordingDeclaredModule;

/// `initialize` against `/mcp`, returning the raw response.
async fn mcp_initialize(app: &TestApp, bearer: Option<&str>) -> TestResponse {
    post_message(app.http(), "/mcp", None, bearer, &initialize_request()).await
}

/// `{ ping }` against `/graphql`, under `headers`.
async fn graphql_ping(app: &TestApp, headers: &[(&str, &str)]) -> TestResponse {
    let mut request = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await
}

/// A graphql-ws upgrade on `/graphql`, under `headers`.
async fn graphql_upgrade(app: &TestApp, headers: &[(&str, &str)]) -> TestResponse {
    let mut request = app
        .http()
        .get("/graphql")
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-protocol", "graphql-transport-ws");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await
}

/// What an upgrade every guard admitted answers in process: `426`, for a
/// request with no connection under it to switch, the step after the guards.
const ADMITTED_UPGRADE: StatusCode = StatusCode::UPGRADE_REQUIRED;

async fn pooled() -> TestApp {
    TestApp::builder()
        .module::<BothTransportsModule>()
        .use_guards_global([guard::<BearerGuard>()])
        .build()
        .await
        .expect("both transports boot under one global pool")
}

#[tokio::test]
async fn a_global_pool_gates_both_transports_for_an_authenticated_caller() {
    let app = pooled().await;

    let gql = graphql_ping(&app, &[("authorization", "Bearer t")]).await;
    gql.assert_status(StatusCode::OK);
    let body = gql.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("pong"),
        "the pooled guard admits an authenticated GraphQL operation: {body}",
    );

    let mcp = mcp_initialize(&app, Some("Bearer t")).await;
    mcp.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn the_pooled_guards_policy_decides_on_mcp_too() {
    let app = pooled().await;

    let denied = mcp_initialize(&app, None).await;
    denied.assert_status(StatusCode::UNAUTHORIZED);
}

// Flipping either half is a security change.
#[tokio::test]
async fn only_graphql_carries_the_in_band_posture() {
    let app = TestApp::builder()
        .module::<BothTransportsModule>()
        .use_guards_global([guard::<PostureProbe>(), guard::<BearerGuard>()])
        .build()
        .await
        .expect("boots");
    let postures = app.container().get::<Postures>().expect("the postures");

    let gql = graphql_ping(&app, &[]).await;
    gql.assert_status(StatusCode::OK);
    let body = gql.0.into_body().into_string().await.expect("body");
    assert!(
        body.contains("pong"),
        "anonymous GraphQL reaches the resolver gates (in band): {body}",
    );
    assert_eq!(postures.take(), [Posture::InBand], "/graphql");

    graphql_upgrade(&app, &[])
        .await
        .assert_status(ADMITTED_UPGRADE);
    assert_eq!(postures.take(), [Posture::InBand], "/graphql upgrade");

    let mcp = mcp_initialize(&app, None).await;
    mcp.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(postures.take(), [Posture::Gated], "/mcp");
}

/// A guard admitting on `is_public()` alone no longer reads `/graphql` as
/// public: the endpoint carries `Posture::InBand`, its operations each declare
/// a posture.
#[tokio::test]
async fn a_public_idiom_guard_checks_graphql() {
    let app = TestApp::builder()
        .module::<BothTransportsModule>()
        .use_guards_global([guard::<PublicOnlyGuard>()])
        .build()
        .await
        .expect("boots");

    graphql_ping(&app, &[])
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    graphql_upgrade(&app, &[])
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn endpoint_guards_add_to_the_pool() {
    let app = TestApp::builder()
        .module::<DeclaredMcpModule>()
        .use_guards_global([guard::<BearerGuard>()])
        .build()
        .await
        .expect("boots");

    mcp_initialize(&app, None)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    mcp_initialize(&app, Some("Bearer t"))
        .await
        .assert_status(StatusCode::OK);

    graphql_ping(&app, &[]).await.assert_status(StatusCode::OK);
}

/// The pool's refusal is answered as the pool's guard raised it, whatever the
/// endpoint's guards decided: here they authenticated the caller.
#[tokio::test]
async fn a_global_http_guard_refuses_graphql_beside_endpoint_guards() {
    let app = TestApp::builder()
        .module::<BearerDeclaredModule>()
        .use_guards_global([guard::<ApiKeyGuard>()])
        .build()
        .await
        .expect("boots");

    graphql_ping(&app, &[("authorization", "Bearer t")])
        .await
        .assert_status(StatusCode::FORBIDDEN);
    graphql_ping(&app, &[("authorization", "Bearer t"), (API_KEY, "k")])
        .await
        .assert_status(StatusCode::OK);
}

/// The graphql-ws upgrade runs the same pool and endpoint guards as the POST,
/// under the same posture: a refusal there opens no socket.
#[tokio::test]
async fn a_global_http_guard_refuses_the_graphql_ws_upgrade_beside_endpoint_guards() {
    let app = TestApp::builder()
        .module::<BearerDeclaredModule>()
        .use_guards_global([guard::<ApiKeyGuard>(), guard::<PostureProbe>()])
        .build()
        .await
        .expect("boots");
    let postures = app.container().get::<Postures>().expect("the postures");

    graphql_upgrade(&app, &[("authorization", "Bearer t")])
        .await
        .assert_status(StatusCode::FORBIDDEN);
    assert!(
        postures.take().is_empty(),
        "refused before the rest of the pool"
    );

    graphql_upgrade(&app, &[("authorization", "Bearer t"), (API_KEY, "k")])
        .await
        .assert_status(ADMITTED_UPGRADE);
    assert_eq!(postures.take(), [Posture::InBand], "past the API key");
}

#[tokio::test]
async fn a_global_http_guard_refuses_mcp_beside_endpoint_guards() {
    let app = TestApp::builder()
        .module::<BearerDeclaredModule>()
        .use_guards_global([guard::<ApiKeyGuard>()])
        .build()
        .await
        .expect("boots");

    mcp_initialize(&app, Some("Bearer t"))
        .await
        .assert_status(StatusCode::FORBIDDEN);
    post_message_with(
        app.http(),
        "/mcp",
        None,
        Some("Bearer t"),
        &[(API_KEY, "k")],
        &initialize_request(),
    )
    .await
    .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn a_guard_in_both_pool_and_endpoint_guards_runs_once() {
    let app = TestApp::builder()
        .module::<RecordingDeclaredModule>()
        .use_guards_global([guard::<CoveredGuard>()])
        .build()
        .await
        .expect("boots");
    let trail = app.container().get::<Trail>().expect("the trail");

    graphql_ping(&app, &[]).await.assert_status(StatusCode::OK);
    assert_eq!(trail.take(), ["covered", "second"], "/graphql");

    mcp_initialize(&app, None)
        .await
        .assert_status(StatusCode::OK);
    assert_eq!(trail.take(), ["covered", "second"], "/mcp");
}

/// The endpoint's guards run where the pool lists the first one they name, so
/// a pooled guard reading the principal still runs after the authentication
/// they carry.
#[tokio::test]
async fn endpoint_guards_run_where_the_pool_lists_the_first_guard_they_name() {
    let app = TestApp::builder()
        .module::<RecordingDeclaredModule>()
        .use_guards_global([
            guard::<FirstGuard>(),
            guard::<CoveredGuard>(),
            guard::<LastGuard>(),
        ])
        .build()
        .await
        .expect("boots");
    let trail = app.container().get::<Trail>().expect("the trail");

    graphql_ping(&app, &[]).await.assert_status(StatusCode::OK);
    assert_eq!(
        trail.take(),
        ["first", "covered", "second", "last"],
        "/graphql"
    );

    graphql_upgrade(&app, &[])
        .await
        .assert_status(ADMITTED_UPGRADE);
    assert_eq!(
        trail.take(),
        ["first", "covered", "second", "last"],
        "/graphql upgrade"
    );

    mcp_initialize(&app, None)
        .await
        .assert_status(StatusCode::OK);
    assert_eq!(trail.take(), ["first", "covered", "second", "last"], "/mcp");
}

#[tokio::test]
async fn with_neither_endpoint_guards_nor_pool_graphql_runs_open_and_mcp_refuses() {
    let app = TestApp::for_module::<BothTransportsModule>()
        .await
        .expect("boots");

    let gql = graphql_ping(&app, &[]).await;
    gql.assert_status(StatusCode::OK);
    let body = gql.0.into_body().into_string().await.expect("body");
    assert!(body.contains("pong"), "graphql has no gate at all: {body}");

    let mcp = mcp_initialize(&app, None).await;
    mcp.assert_status(StatusCode::UNAUTHORIZED);
}
