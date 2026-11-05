//! Covers `src/subject.rs` — a subject no table holds, declared with
//! `#[subject]`: `#[authorize(Manage, Exporting)]` runs the class gate on every
//! edge, with its refusals and RFC 6750 §3.1's scope challenge, and arms no
//! mask, so a reply no entity could reconcile goes out as written.

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, Manage, Read, current_ability, subject};
use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, Scope};
use nest_rs_http::futures_util::stream;
use nest_rs_http::{HttpView, Json, SseEvent, SseStream, StatusCode, controller, header, routes};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::call_tool_as;
use nest_rs_ws::{WsError, WsModule, gateway, messages};

/// May run the org's exports.
#[subject]
struct Exporting;

/// The scope a token must carry for the grant to materialize.
const EXPORTS_RUN: Scope = Scope::new("exports:run");

/// RFC 6750 §3.1's description of `insufficient_scope`, the one sentence.
const TOO_NARROW: &str = "the request requires higher privileges than provided by the access token";

/// The header naming which credential [`Grants`] stands for.
const GRANT: &str = "x-grant";

/// The ability of the credential `grant` names: `full` carries the scope,
/// `narrow` delegated none, anything else holds no grant on the capability.
fn ability_of(grant: Option<&str>) -> Ability {
    let scopes: Arc<[String]> = match grant {
        Some("full") => Arc::from([String::from(EXPORTS_RUN)]),
        _ => Arc::from([]),
    };
    let mut builder = AbilityBuilder::new().with_granted_scopes(Some(scopes));
    if matches!(grant, Some("full" | "narrow")) {
        builder
            .can(Action::Manage, Exporting)
            .requires_scope(EXPORTS_RUN);
    }
    builder.build().expect("valid test ability")
}

/// Attaches the ability of the credential the request names, naming its
/// caller as the authentication guard does.
#[injectable]
#[derive(Default)]
struct Grants;

impl Layer for Grants {}

#[async_trait]
impl Guard for Grants {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let grant = HttpView::of(cx)?
            .request()
            .headers()
            .get(GRANT)
            .and_then(|grant| grant.to_str().ok())
            .map(str::to_owned);
        nest_rs_core::__private::set_actor_id("exporter");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability_of(
            grant.as_deref(),
        )));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for Grants {}

/// What the export route answers: no entity's row, which a mask armed on one
/// could only fail closed on.
#[derive(serde::Serialize, schemars::JsonSchema)]
struct Queued {
    queued: bool,
    reads_scoped: bool,
}

#[controller(path = "/exports")]
#[derive(Default)]
struct ExportsController;

#[routes]
impl ExportsController {
    #[post("/")]
    #[authorize(Manage, Exporting)]
    async fn run(&self) -> Json<Queued> {
        Json(Queued {
            queued: true,
            reads_scoped: current_ability().is_some(),
        })
    }

    /// The opening on a capability: it reads the caller's grant, and shapes
    /// nothing.
    #[get("/catalog")]
    #[public(Read, Exporting)]
    async fn catalog(&self) -> String {
        "catalog".to_owned()
    }

    #[sse("/progress")]
    #[authorize(Manage, Exporting)]
    async fn progress(&self) -> SseStream {
        SseStream::new(stream::iter([
            SseEvent::message("done").event_type("export")
        ]))
    }
}

/// A GraphQL answer with no serde impl: a gate-only posture asks nothing of it.
#[derive(SimpleObject)]
struct ExportStatus {
    running: bool,
}

#[resolver]
struct ExportsResolver;

#[operations]
impl ExportsResolver {
    #[query]
    #[authorize(Manage, Exporting)]
    async fn export_status(&self) -> GqlResult<ExportStatus> {
        Ok(ExportStatus { running: true })
    }
}

#[mcp(path = "/mcp")]
#[derive(Clone, Default)]
struct ExportsTool;

#[tools]
impl ExportsTool {
    /// Run the org's export.
    #[tool]
    #[authorize(Manage, Exporting)]
    async fn run_export(&self) -> Result<String, McpError> {
        Ok("export started".to_owned())
    }
}

#[gateway(path = "/ws/exports")]
#[derive(Default)]
struct ExportsGateway;

#[messages]
impl ExportsGateway {
    #[subscribe_message("run")]
    #[authorize(Manage, Exporting)]
    async fn run(&self) -> Result<String, WsError> {
        Ok("export started".to_owned())
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [Grants, ExportsController, ExportsResolver, ExportsTool],
)]
struct ExportsModule;

#[module(imports = [WsModule], providers = [Grants, ExportsGateway])]
struct ExportsWsModule;

#[module(imports = [ExportsModule], providers = [GlobalGuards<(Grants,)>])]
struct ExportsApp;

async fn app() -> TestApp {
    TestApp::builder()
        .module::<ExportsApp>()
        .build()
        .await
        .expect("every edge boots")
}

/// What each edge answers the credential `grant` names: the HTTP route's
/// response, the stream's, the GraphQL document, the MCP body and the WS frame.
struct Answers {
    http: nest_rs_http::Response,
    sse: nest_rs_http::Response,
    graphql: serde_json::Value,
    mcp: String,
    ws: serde_json::Value,
}

#[module(imports = [ExportsWsModule], providers = [GlobalGuards<(Grants,)>])]
struct AnswersApp;

async fn answers(grant: &str) -> Answers {
    let app = app().await;
    let http = app
        .http()
        .post("/exports")
        .header(GRANT, grant)
        .send()
        .await
        .0;
    let sse = app
        .http()
        .get("/exports/progress")
        .header(GRANT, grant)
        .send()
        .await
        .0;
    let graphql = serde_json::to_value(
        app.http()
            .post("/graphql")
            .header(GRANT, grant)
            .body_json(&serde_json::json!({ "query": "{ exportStatus { running } }" }))
            .send()
            .await
            .json()
            .await,
    )
    .expect("a GraphQL response");
    let mcp = call_tool_as(
        app.http(),
        "/mcp",
        "run_export",
        None,
        &[(GRANT, grant)],
        serde_json::json!({}),
    )
    .await;

    let ws = TestApp::builder()
        .module::<AnswersApp>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let mut socket = ws
        .socket("/ws/exports")
        .header(GRANT, grant)
        .connect()
        .await;
    socket.send("run", serde_json::Value::Null).await;
    let frame = socket.next_envelope().await;
    ws.shutdown().await.expect("the transport stops cleanly");

    Answers {
        http,
        sse,
        graphql,
        mcp,
        ws: frame,
    }
}

fn challenge(response: &nest_rs_http::Response) -> String {
    response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn body(response: nest_rs_http::Response) -> String {
    response
        .into_body()
        .into_string()
        .await
        .expect("a readable body")
}

#[tokio::test]
async fn a_granted_caller_runs_the_capability_on_every_edge_and_no_mask_touches_the_reply() {
    let Answers {
        http,
        sse,
        graphql,
        mcp,
        ws,
    } = answers("full").await;

    assert_eq!(http.status(), StatusCode::OK);
    let reply: serde_json::Value =
        serde_json::from_str(&body(http).await).expect("the route's JSON");
    assert_eq!(
        reply,
        serde_json::json!({ "queued": true, "reads_scoped": false }),
        "the reply goes out as written, and a route no mask shapes withholds the ability \
         from its reads",
    );

    assert_eq!(sse.status(), StatusCode::OK, "the gate admits the stream");
    let events = body(sse).await;
    assert!(events.contains("event: export"), "{events}");

    assert_eq!(
        graphql["data"]["exportStatus"]["running"], true,
        "GraphQL: {graphql}"
    );
    assert!(mcp.contains("export started"), "MCP: {mcp}");
    assert_eq!(ws["data"], "export started", "WS: {ws}");
}

#[tokio::test]
async fn a_principal_without_the_grant_is_forbidden_on_every_edge_with_no_scope_to_ask_for() {
    let Answers {
        http,
        sse,
        graphql,
        mcp,
        ws,
    } = answers("none").await;

    for (edge, refused) in [("HTTP", http), ("SSE", sse)] {
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{edge}");
        assert!(
            !challenge(&refused).contains("insufficient_scope"),
            "{edge}: no wider token would serve it"
        );
        let problem: serde_json::Value =
            serde_json::from_str(&body(refused).await).expect("a problem document");
        assert_eq!(problem["code"], "FORBIDDEN", "{edge}: {problem}");
    }
    assert_eq!(
        graphql["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "GraphQL: {graphql}"
    );
    assert!(mcp.contains("\"code\":\"FORBIDDEN\""), "MCP: {mcp}");
    assert!(!mcp.contains("export started"), "MCP: {mcp}");
    assert_eq!(ws["data"]["code"], "FORBIDDEN", "WS: {ws}");
}

#[tokio::test]
async fn a_token_without_the_scope_is_challenged_for_it_on_every_edge() {
    let Answers {
        http,
        sse,
        graphql,
        mcp,
        ws,
    } = answers("narrow").await;

    for (edge, refused) in [("HTTP", http), ("SSE", sse)] {
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{edge}");
        assert_eq!(
            challenge(&refused),
            r#"Bearer error="insufficient_scope", scope="exports:run""#,
            "{edge}"
        );
        let problem: serde_json::Value =
            serde_json::from_str(&body(refused).await).expect("a problem document");
        assert_eq!(problem["detail"], TOO_NARROW, "{edge}: {problem}");
    }
    let refused = &graphql["errors"][0];
    assert_eq!(refused["message"], TOO_NARROW, "GraphQL: {graphql}");
    assert_eq!(
        refused["extensions"]["requiredScopes"],
        serde_json::json!([EXPORTS_RUN.as_str()]),
        "GraphQL: {graphql}",
    );
    assert!(
        mcp.contains(r#""requiredScopes":["exports:run"]"#),
        "MCP: {mcp}"
    );
    assert_eq!(
        ws["data"]["requiredScopes"],
        serde_json::json!([EXPORTS_RUN.as_str()]),
        "WS: {ws}"
    );
}

#[tokio::test]
async fn a_public_route_naming_the_capability_admits_a_grant_and_shapes_nothing() {
    let app = app().await;
    let read = |grant: &'static str| {
        app.http()
            .get("/exports/catalog")
            .header(GRANT, grant)
            .send()
    };

    let granted = read("full").await.0;
    assert_eq!(granted.status(), StatusCode::OK);
    assert_eq!(body(granted).await, "catalog");
    assert_eq!(read("none").await.0.status(), StatusCode::FORBIDDEN);
}
