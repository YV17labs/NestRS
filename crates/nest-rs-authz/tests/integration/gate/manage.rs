//! A site gated on `Manage` means every concrete action, on every edge through
//! its real decorator: a denial of one refuses it, four separate grants pass it,
//! and what it answers is what the caller may `Read`. `#[public(Manage, E)]`
//! holds a visitor's rules to the same reading.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authz::{AbilityBuilder, Action, Manage};
use nest_rs_core::{Layer, Module, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::{HttpView, Json, controller, routes};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::mcp::call_tool_statelessly;
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{WsError, gateway, messages};

use crate::widget;

const FORBIDDEN: &str = r#""code":"FORBIDDEN""#;

/// The bodies a refused caller must not reach.
static BODIES: AtomicUsize = AtomicUsize::new(0);

/// A widget as every edge answers it; `name` is what a mask may take off.
#[derive(SimpleObject, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct ManagedWidget {
    id: i32,
    name: Option<String>,
}

/// What every site answers: two rows, told apart by their names.
fn managed_widgets() -> Vec<ManagedWidget> {
    BODIES.fetch_add(1, Ordering::SeqCst);
    vec![
        ManagedWidget {
            id: 1,
            name: Some("first".to_owned()),
        },
        ManagedWidget {
            id: 2,
            name: Some("second".to_owned()),
        },
    ]
}

/// The rules a role names, built for a caller or for a visitor.
fn rules_of(role: &str) -> AbilityBuilder {
    let mut builder = AbilityBuilder::new();
    match role {
        "wildcard" => {
            builder.can(Action::Manage, widget::Entity);
        }
        "each_action" => {
            for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
                builder.can(action, widget::Entity);
            }
        }
        "no_delete" => {
            for action in [Action::Read, Action::Create, Action::Update] {
                builder.can(action, widget::Entity);
            }
        }
        "read_denied" => {
            builder.can(Action::Manage, widget::Entity);
            builder.cannot(Action::Read, widget::Entity);
        }
        "update_denied" => {
            builder.can(Action::Manage, widget::Entity);
            builder.cannot(Action::Update, widget::Entity);
        }
        "narrow_wildcard" => {
            builder
                .can(Action::Manage, widget::Entity)
                .fields([widget::Column::Id]);
            builder
                .can(Action::Read, widget::Entity)
                .fields([widget::Column::Id, widget::Column::Name]);
        }
        "second_denied" => {
            builder.can(Action::Manage, widget::Entity);
            builder
                .cannot(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 2));
        }
        "second_delete_denied" => {
            builder.can(Action::Manage, widget::Entity);
            builder
                .cannot(Action::Delete, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 2));
        }
        other => unreachable!("no such role: {other}"),
    }
    builder
}

/// The role a request names.
fn role_of(cx: &mut UnitContext<'_>) -> Result<String, Denial> {
    let role = HttpView::of(cx)?.request().header("x-role");
    Ok(role.unwrap_or_default().to_owned())
}

/// Attaches the ability of the role the request names.
#[injectable]
#[derive(Default)]
struct ManagerToken;

impl Layer for ManagerToken {}

#[async_trait]
impl Guard for ManagerToken {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = rules_of(&role_of(cx)?).build().expect("valid test ability");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for ManagerToken {}

/// Attaches the visitor's rules the request names. It declares no phase and so
/// identifies no one: `#[public]` is what opens a site beneath it, on every edge.
#[injectable]
#[derive(Default)]
struct VisitorRules;

impl Layer for VisitorRules {}

#[async_trait]
impl Guard for VisitorRules {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = rules_of(&role_of(cx)?)
            .build_visitor()
            .expect("valid test ability");
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for VisitorRules {}

#[controller(path = "/managed")]
#[derive(Default)]
struct ManagedController;

#[routes]
impl ManagedController {
    #[get("/")]
    #[authorize(Manage, widget::Entity)]
    async fn list(&self) -> Json<Vec<ManagedWidget>> {
        Json(managed_widgets())
    }
}

#[resolver]
struct ManagedResolver;

#[operations]
impl ManagedResolver {
    #[query]
    #[authorize(Manage, widget::Entity)]
    async fn managed_widgets(&self) -> GqlResult<Vec<ManagedWidget>> {
        Ok(managed_widgets())
    }
}

#[mcp(path = "/mcp/managed")]
#[derive(Clone, Default)]
struct ManagedTool;

#[tools]
impl ManagedTool {
    /// The widgets a manager answers for.
    #[tool]
    #[authorize(Manage, widget::Entity)]
    async fn managed_widgets(&self) -> Result<nest_rs_mcp::Json<Vec<ManagedWidget>>, McpError> {
        Ok(nest_rs_mcp::Json(managed_widgets()))
    }
}

#[gateway(path = "/ws/managed")]
#[derive(Default)]
struct ManagedGateway;

#[messages]
impl ManagedGateway {
    #[subscribe_message("list")]
    #[authorize(Manage, widget::Entity)]
    async fn list(&self) -> Result<Vec<ManagedWidget>, WsError> {
        Ok(managed_widgets())
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [
        ManagerToken,
        GlobalGuards<(ManagerToken,)>,
        ManagedController,
        ManagedResolver,
        ManagedTool,
    ],
)]
struct ManagedEveryEdgeApp;

#[module(
    imports = [nest_rs_ws::WsModule],
    providers = [ManagerToken, GlobalGuards<(ManagerToken,)>, ManagedGateway],
)]
struct ManagedWsApp;

#[controller(path = "/managed")]
#[derive(Default)]
struct PublicController;

#[routes]
impl PublicController {
    #[get("/")]
    #[public(Manage, widget::Entity)]
    async fn list(&self) -> Json<Vec<ManagedWidget>> {
        Json(managed_widgets())
    }
}

#[resolver]
struct PublicResolver;

#[operations]
impl PublicResolver {
    #[query]
    #[public(Manage, widget::Entity)]
    async fn managed_widgets(&self) -> GqlResult<Vec<ManagedWidget>> {
        Ok(managed_widgets())
    }
}

#[mcp(path = "/mcp/managed")]
#[derive(Clone, Default)]
struct PublicTool;

#[tools]
impl PublicTool {
    /// The widgets a visitor answers for.
    #[tool]
    #[public(Manage, widget::Entity)]
    async fn managed_widgets(&self) -> Result<nest_rs_mcp::Json<Vec<ManagedWidget>>, McpError> {
        Ok(nest_rs_mcp::Json(managed_widgets()))
    }
}

#[gateway(path = "/ws/managed")]
#[derive(Default)]
struct PublicGateway;

#[messages]
impl PublicGateway {
    #[subscribe_message("list")]
    #[public(Manage, widget::Entity)]
    async fn list(&self) -> Result<Vec<ManagedWidget>, WsError> {
        Ok(managed_widgets())
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [
        VisitorRules,
        GlobalGuards<(VisitorRules,)>,
        PublicController,
        PublicResolver,
        PublicTool,
    ],
)]
struct PublicEveryEdgeApp;

#[module(
    imports = [nest_rs_ws::WsModule],
    providers = [VisitorRules, GlobalGuards<(VisitorRules,)>, PublicGateway],
)]
struct PublicWsApp;

/// What `role` reads at the `#[authorize(Manage, E)]` site of each edge.
async fn answers(role: &str) -> [(&'static str, String); 4] {
    answers_of::<ManagedEveryEdgeApp, ManagedWsApp>(role).await
}

/// What a visitor's `role` reads at the `#[public(Manage, E)]` site of each edge.
async fn visitors_answers(role: &str) -> [(&'static str, String); 4] {
    answers_of::<PublicEveryEdgeApp, PublicWsApp>(role).await
}

/// What `role` reads at the one site of each edge the two apps serve, by the
/// edge's name.
async fn answers_of<App: Module + 'static, Ws: Module + 'static>(
    role: &str,
) -> [(&'static str, String); 4] {
    let app = TestApp::builder()
        .module::<App>()
        .build()
        .await
        .expect("every edge boots");
    let ws = TestApp::builder()
        .module::<Ws>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let http = app
        .http()
        .get("/managed")
        .header("x-role", role)
        .send()
        .await;
    let http = http.0.into_body().into_string().await.expect("a body");
    let graphql = crate::graphql::query(&app, role, "{ managedWidgets { id name } }")
        .await
        .to_string();
    let mcp = call_tool_statelessly(
        app.http(),
        "/mcp/managed",
        "managed_widgets",
        None,
        &[("x-role", role)],
        serde_json::json!({}),
    )
    .await;
    let frame = crate::ws::message_as(&ws, "/ws/managed", Some(role), "list").await;
    ws.shutdown().await.expect("the transport stops cleanly");
    [
        ("HTTP", http),
        ("GraphQL", graphql),
        ("MCP", mcp),
        ("WS", frame.to_string()),
    ]
}

#[tokio::test]
async fn a_denial_or_an_absent_grant_of_one_action_refuses_it_on_every_edge_and_runs_no_body() {
    for role in ["read_denied", "no_delete"] {
        for (edge, answer) in answers(role).await {
            assert!(answer.contains(FORBIDDEN), "{edge}, {role}: {answer}");
            assert!(
                !answer.contains("first"),
                "{edge}, {role}: no row: {answer}"
            );
        }
    }
    assert_eq!(
        BODIES.load(Ordering::SeqCst),
        0,
        "a body ran for a caller a site gated on `Manage` refuses",
    );
}

#[tokio::test]
async fn the_wildcard_and_a_grant_of_each_action_pass_it_on_every_edge() {
    for role in ["wildcard", "each_action"] {
        for (edge, answer) in answers(role).await {
            assert!(
                answer.contains("first") && answer.contains("second"),
                "{edge}, {role}: {answer}",
            );
        }
    }
}

#[tokio::test]
async fn its_reply_shows_the_columns_the_caller_may_read_on_every_edge() {
    for (edge, answer) in answers("narrow_wildcard").await {
        assert!(
            answer.contains("first"),
            "{edge}: `Read`'s grant opens the name the wildcard's leaves out: {answer}",
        );
    }
}

#[tokio::test]
async fn its_reply_drops_a_row_read_refuses_on_every_edge() {
    for (edge, answer) in answers("second_denied").await {
        assert!(answer.contains("first"), "{edge}: {answer}");
        assert!(
            !answer.contains("second"),
            "{edge}: `cannot(Read)` holds at a site gated on `Manage`: {answer}",
        );
    }
}

#[tokio::test]
async fn its_reply_keeps_a_row_only_another_action_refuses_on_every_edge() {
    for (edge, answer) in answers("second_delete_denied").await {
        assert!(
            answer.contains("first") && answer.contains("second"),
            "{edge}: a reply reads `Read`, and `cannot(Delete)` is no part of it: {answer}",
        );
    }
}

#[tokio::test]
async fn a_visitors_denial_of_one_action_refuses_a_public_site_on_every_edge_and_runs_no_body() {
    let logs = LogCapture::install();
    let roles = ["read_denied", "update_denied"];
    for role in roles {
        for (edge, answer) in visitors_answers(role).await {
            assert!(answer.contains(FORBIDDEN), "{edge}, {role}: {answer}");
        }
    }
    assert_eq!(
        BODIES.load(Ordering::SeqCst),
        0,
        "a body ran for a visitor a site gated on `Manage` refuses",
    );
    let filed = logs.find(nest_rs_authz::TARGET, "authorization denied");
    assert_eq!(
        filed.len(),
        roles.len() * 4,
        "one line per refusal: {filed:#?}"
    );
    for line in filed {
        assert_eq!(line.field("reason").as_deref(), Some("class_denied"));
    }
}

#[tokio::test]
async fn a_visitors_denial_of_read_a_when_narrows_drops_its_row_at_a_public_site_on_every_edge() {
    for (edge, answer) in visitors_answers("second_denied").await {
        assert!(answer.contains("first"), "{edge}: {answer}");
        assert!(
            !answer.contains("second"),
            "{edge}: `cannot(Read)` holds for a visitor at `#[public(Manage, E)]`: {answer}",
        );
    }
}
