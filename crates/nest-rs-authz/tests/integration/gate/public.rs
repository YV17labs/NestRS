//! `#[public(Action, Subject)]`, through each edge's real decorator: what a
//! visitor reads is a line a reviewer reads, so its rows and its fields are the
//! visitor's grant's on HTTP, GraphQL, WS and MCP alike, and a bare `#[public]`
//! reads nothing a grant guards. Each handler answers the rows a `Repo` read
//! would, and says whether its reads would run under an ability: the live read
//! is `nest-rs-seaorm`'s e2e suite.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Read, current_ability};
use nest_rs_core::{AmbientValue, Discovery, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::__private::route_denials;
use nest_rs_http::{
    EndpointGuards, HttpControllerMeta, HttpView, Json, StatusCode, controller, routes,
};
use nest_rs_mcp::model::{GetPromptResult, PromptMessage, Resource, Role};
use nest_rs_mcp::{McpAccess, McpError, McpModule, mcp, tools};
use nest_rs_testing::mcp::{call_method, call_tool, open_session};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_ws::{WsError, WsModule, gateway, messages};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::graphql::query;

/// What a visitor may read: published articles, their title alone.
mod article {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Article")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "articles")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub title: String,
        #[expose]
        pub published: bool,
        #[expose]
        pub body: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// What no visitor grant reaches.
mod sealed {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Sealed")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "sealed")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A capability no table holds, which a visitor's grant reaches.
#[nest_rs_authz::subject]
struct Browsing;

/// An article as every edge answers it: each member optional, so a field grant
/// has a representation in a typed answer.
#[derive(Clone, Debug, Serialize, Deserialize, SimpleObject, schemars::JsonSchema)]
#[graphql(complex)]
struct ArticleDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

/// The rows a read answers: two published, one draft.
fn rows() -> Vec<ArticleDto> {
    [(1, true), (2, true), (3, false)]
        .into_iter()
        .map(|(id, published)| ArticleDto {
            id: Some(id),
            title: Some(format!("article {id}")),
            published: Some(published),
            body: Some(format!("body {id}")),
        })
        .collect()
}

/// The visitor's answer on an edge dropping what its grant strips.
fn titles() -> Value {
    json!([{ "title": "article 1" }, { "title": "article 2" }])
}

/// A caller a token backs.
#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

/// Stands in for the authentication guard: a token attaches [`Caller`]; none
/// admits a visitor on what `#[public]` opens and at an in-band request.
#[injectable]
#[derive(Default)]
struct CallerGuard;

impl Layer for CallerGuard {}

#[async_trait]
impl Guard for CallerGuard {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<Caller>())
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        if HttpView::of(cx)?.request().header("x-token").is_some() {
            cx.attach(Caller);
        }
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for CallerGuard {}

#[injectable]
#[derive(Default)]
struct Rules;

impl AbilityFactory for Rules {
    type Actor = Caller;

    fn define(&self, _caller: &Caller, ability: &mut AbilityBuilder) {
        ability.can(Action::Read, article::Entity);
    }

    fn define_visitor(&self, ability: &mut AbilityBuilder) {
        ability
            .can(Action::Read, article::Entity)
            .when(|p| p.eq(article::Column::Published, true))
            .fields([article::Column::Title]);
        ability.can(Action::Read, Browsing);
    }
}

type AuthzGuard = AbilityGuard<Rules>;

/// Whether the running unit's reads would run under an ability.
fn reads_under_an_ability() -> bool {
    current_ability().is_some()
}

#[controller(path = "/articles")]
#[use_guards(CallerGuard, AuthzGuard)]
#[derive(Default)]
struct ArticlesController;

#[routes]
impl ArticlesController {
    #[get("/")]
    #[public(Read, article::Entity)]
    async fn feed(&self) -> Json<Vec<ArticleDto>> {
        Json(rows())
    }

    #[get("/bare")]
    #[public]
    async fn bare(&self) -> Json<bool> {
        Json(reads_under_an_ability())
    }

    #[get("/sealed")]
    #[public(Read, sealed::Entity)]
    async fn sealed(&self) -> Json<Vec<i32>> {
        Json(Vec::new())
    }

    #[get("/browsing")]
    #[public(Read, Browsing)]
    async fn browsing(&self) -> Json<bool> {
        Json(reads_under_an_ability())
    }
}

#[resolver]
struct ArticlesResolver;

#[operations]
impl ArticlesResolver {
    #[query]
    #[public(Read, article::Entity)]
    async fn feed(&self) -> GqlResult<Vec<ArticleDto>> {
        Ok(rows())
    }

    #[query]
    #[public(Read, article::Entity, unmasked)]
    async fn feed_by_hand(&self) -> GqlResult<bool> {
        Ok(reads_under_an_ability())
    }

    #[query]
    #[public]
    async fn bare(&self) -> GqlResult<bool> {
        Ok(reads_under_an_ability())
    }

    #[query]
    #[public(Read, sealed::Entity)]
    async fn sealed(&self) -> GqlResult<Vec<i32>> {
        Ok(Vec::new())
    }

    #[query]
    #[public(Read, Browsing)]
    async fn browsing(&self) -> GqlResult<bool> {
        Ok(reads_under_an_ability())
    }

    /// Each item is masked as a row: a draft dropped, a published one its title.
    #[subscription]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn feed_stream(&self) -> impl Stream<Item = ArticleDto> {
        stream::iter(rows())
    }

    /// A field no mask reaches, below an operation naming its read.
    #[field_resolver]
    async fn nested_read(&self, parent: &ArticleDto) -> GqlResult<bool> {
        let _ = parent;
        Ok(reads_under_an_ability())
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [
        Rules,
        CallerGuard,
        AuthzGuard,
        ArticlesController,
        ArticlesResolver,
        EndpointGuards<GraphqlView, (CallerGuard, AuthzGuard)>,
    ],
)]
struct ArticlesApp;

async fn app() -> TestApp {
    TestApp::for_module::<ArticlesApp>()
        .await
        .expect("the routes and the resolver boot")
}

async fn body(response: nest_rs_http::Response) -> Value {
    let text = response
        .into_body()
        .into_string()
        .await
        .expect("a readable body");
    serde_json::from_str(&text).expect("a JSON body")
}

#[tokio::test]
async fn a_route_naming_its_read_answers_a_visitor_by_its_grant_and_a_token_by_define() {
    let app = app().await;

    let visitor = app.http().get("/articles").send().await.0;
    assert_eq!(visitor.status(), StatusCode::OK);
    assert_eq!(body(visitor).await, titles());

    let caller = app
        .http()
        .get("/articles")
        .header("x-token", "valid")
        .send()
        .await
        .0;
    assert_eq!(caller.status(), StatusCode::OK);
    let read = body(caller).await;
    assert_eq!(read.as_array().map(Vec::len), Some(3), "{read}");
    assert_eq!(read[2]["body"], "body 3", "{read}");
}

/// What a route's document reads: `#[public(...)]` is public and masked, its gate
/// answering `403` and no `401`, as the parameter it replaces did.
#[tokio::test]
async fn a_route_naming_its_read_documents_a_public_masked_operation() {
    let app = app().await;
    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let feed = controllers
        .iter()
        .flat_map(|found| found.meta.routes())
        .find(|route| route.handler() == "feed")
        .expect("the route is discovered");
    assert!(feed.public() && feed.masked());
    let statuses: Vec<u16> = route_denials(app.container(), feed)
        .expect("the site composed")
        .into_iter()
        .map(|(status, _)| status)
        .collect();
    assert_eq!(statuses, [403]);
}

#[tokio::test]
async fn a_bare_public_route_reads_under_no_ability_whoever_calls_it() {
    let app = app().await;
    for token in [None, Some("valid")] {
        let mut read = app.http().get("/articles/bare");
        if let Some(token) = token {
            read = read.header("x-token", token);
        }
        let answer = read.send().await.0;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(body(answer).await, json!(false), "{token:?}");
    }
}

#[tokio::test]
async fn a_route_a_visitors_grant_does_not_reach_is_forbidden() {
    let logs = LogCapture::install();
    let app = app().await;

    let refused = app.http().get("/articles/sealed").send().await.0;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(body(refused).await["code"], "FORBIDDEN");
    let filed = logs.expect_one("nest_rs::authz", "authorization denied");
    assert_eq!(filed.field("reason").as_deref(), Some("no_class_grant"));
}

#[tokio::test]
async fn a_route_naming_a_capability_gates_and_arms_no_mask() {
    let app = app().await;
    let answer = app.http().get("/articles/browsing").send().await.0;
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(
        body(answer).await,
        json!(false),
        "a capability has no rows, so what a route reads stays withheld",
    );
}

#[tokio::test]
async fn an_operation_naming_its_read_answers_a_visitor_by_its_grant_and_a_token_by_define() {
    let app = app().await;
    let document = "{ feed { title body } }";

    let visitor = query(&app, "", document).await;
    assert_eq!(
        visitor["data"]["feed"],
        json!([
            { "title": "article 1", "body": null },
            { "title": "article 2", "body": null },
        ]),
        "{visitor}",
    );

    let caller = app
        .http()
        .post("/graphql")
        .header("x-token", "valid")
        .body_json(&json!({ "query": document }))
        .send()
        .await
        .json()
        .await;
    let read = serde_json::to_value(caller).expect("a GraphQL response");
    assert_eq!(
        read["data"]["feed"].as_array().map(Vec::len),
        Some(3),
        "{read}"
    );
    assert_eq!(read["data"]["feed"][2]["body"], "body 3", "{read}");
}

#[tokio::test]
async fn an_operation_leaving_the_mask_to_its_body_still_reads_under_the_visitors_rules() {
    let answer = query(&app().await, "", "{ feedByHand }").await;
    assert_eq!(answer["data"]["feedByHand"], true, "{answer}");
}

#[tokio::test]
async fn a_bare_public_operation_reads_under_no_ability_whoever_calls_it() {
    let app = app().await;
    let visitor = query(&app, "", "{ bare }").await;
    assert_eq!(visitor["data"]["bare"], false, "{visitor}");

    let caller = app
        .http()
        .post("/graphql")
        .header("x-token", "valid")
        .body_json(&json!({ "query": "{ bare }" }))
        .send()
        .await
        .json()
        .await;
    let read = serde_json::to_value(caller).expect("a GraphQL response");
    assert_eq!(read["data"]["bare"], false, "{read}");
}

#[tokio::test]
async fn an_operation_a_visitors_grant_does_not_reach_answers_forbidden() {
    let refused = query(&app().await, "", "{ sealed }").await;
    assert_eq!(
        refused["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{refused}",
    );
    assert_eq!(refused["errors"][0]["message"], "forbidden", "{refused}");
}

#[tokio::test]
async fn an_operation_naming_a_capability_gates_and_arms_no_mask() {
    let answer = query(&app().await, "", "{ browsing }").await;
    assert_eq!(answer["data"]["browsing"], false, "{answer}");
}

#[tokio::test]
async fn a_field_below_a_public_operation_reads_nothing_for_a_visitor() {
    let app = app().await;
    let document = "{ feed { title nestedRead } }";

    let visitor = query(&app, "", document).await;
    assert_eq!(
        visitor["data"]["feed"][0]["nestedRead"], false,
        "no mask reaches a nested field, so a visitor's rules read nothing there: {visitor}",
    );

    let caller = app
        .http()
        .post("/graphql")
        .header("x-token", "valid")
        .body_json(&json!({ "query": document }))
        .send()
        .await
        .json()
        .await;
    let read = serde_json::to_value(caller).expect("a GraphQL response");
    assert_eq!(read["data"]["feed"][0]["nestedRead"], true, "{read}");
}

#[tokio::test]
async fn a_subscription_naming_its_read_masks_each_item_by_the_visitors_grant() {
    let app = app().await;
    let mut socket = app.graphql_socket().open();
    socket.connect().await;
    socket.subscribe("1", "subscription { feedStream { title body } }");
    for title in ["article 1", "article 2"] {
        let item = socket.next_item("1").await.expect("a published article");
        assert_eq!(
            item["data"]["feedStream"],
            json!({ "title": title, "body": null }),
            "{item}",
        );
    }
    let done = socket.next_item("1").await.expect("the stream ends");
    assert_eq!(done["type"], "complete", "the draft was dropped: {done}");
}

#[gateway(path = "/ws/articles")]
#[derive(Default)]
struct ArticlesGateway;

#[messages]
impl ArticlesGateway {
    #[subscribe_message("feed")]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn feed(&self) -> Result<Vec<ArticleDto>, WsError> {
        Ok(rows())
    }

    #[subscribe_message("bare")]
    #[public]
    #[use_guards(AuthzGuard)]
    async fn bare(&self) -> bool {
        reads_under_an_ability()
    }

    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    #[subscribe_message("sealed")]
    #[public(Read, sealed::Entity)]
    #[use_guards(AuthzGuard)]
    async fn sealed(&self) -> Result<Vec<i32>, WsError> {
        Ok(Vec::new())
    }

    #[subscribe_message("browsing")]
    #[public(Read, Browsing)]
    #[use_guards(AuthzGuard)]
    async fn browsing(&self) -> bool {
        reads_under_an_ability()
    }
}

#[module(imports = [WsModule], providers = [Rules, AuthzGuard, ArticlesGateway])]
struct ArticlesWsApp;

/// What the gateway answers `event`, a visitor having connected.
async fn message(event: &str) -> Value {
    let app = TestApp::builder()
        .module::<ArticlesWsApp>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let mut socket = app.socket("/ws/articles").connect().await;
    socket.send(event, Value::Null).await;
    let frame = socket.next_envelope().await;
    app.shutdown().await.expect("the transport stops cleanly");
    frame
}

#[tokio::test]
async fn a_message_naming_its_read_answers_a_visitor_by_its_grant() {
    assert_eq!(message("feed").await["data"], titles());
}

/// The gate alone admits the read as the site does, so a handler a test calls on
/// its own reads what it would read on a socket.
#[tokio::test]
async fn a_message_dispatched_on_its_own_masks_by_the_visitors_grant_too() {
    let mut visitor = AbilityBuilder::new();
    visitor
        .can(Action::Read, article::Entity)
        .when(|p| p.eq(article::Column::Published, true))
        .fields([article::Column::Title]);
    let visitor = visitor.build_visitor().expect("one grant builds");
    let reply = crate::ws::dispatch_with(&ArticlesGateway, visitor.into(), "feed").await;
    assert_eq!(
        serde_json::from_str::<Value>(&crate::ws::body(reply)).expect("the reply's JSON"),
        titles(),
    );
}

#[tokio::test]
async fn a_bare_public_message_reads_under_no_ability() {
    assert_eq!(message("bare").await["data"], false);
}

#[tokio::test]
async fn a_bare_public_message_reading_no_entity_answers_as_before() {
    assert_eq!(message("ping").await["data"], "pong");
}

#[tokio::test]
async fn a_message_a_visitors_grant_does_not_reach_answers_forbidden() {
    let refused = message("sealed").await;
    assert_eq!(refused["data"]["code"], "FORBIDDEN", "{refused}");
    assert_eq!(refused["data"]["error"], "forbidden", "{refused}");
}

#[tokio::test]
async fn a_message_naming_a_capability_gates_and_arms_no_mask() {
    assert_eq!(message("browsing").await["data"], false);
}

const MCP_PATH: &str = "/mcp/articles";

#[mcp(path = "/mcp/articles")]
#[use_guards(AuthzGuard)]
#[derive(Clone, Default)]
struct ArticlesTool;

#[tools]
impl ArticlesTool {
    /// The articles a visitor may read.
    #[tool]
    #[public(Read, article::Entity)]
    async fn feed(&self) -> Result<nest_rs_mcp::Json<Vec<ArticleDto>>, McpError> {
        Ok(nest_rs_mcp::Json(rows()))
    }

    /// Whether this tool's reads run under an ability.
    #[tool]
    #[public]
    async fn bare(&self) -> Result<String, McpError> {
        Ok(reads_under_an_ability().to_string())
    }

    /// A tool reading nothing.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }

    /// What no visitor grant reaches.
    #[tool]
    #[public(Read, sealed::Entity)]
    async fn sealed(&self) -> Result<String, McpError> {
        Ok("reached".to_owned())
    }

    /// One article, read through its template.
    #[resource(template = "article://{id}")]
    #[public(Read, article::Entity)]
    async fn article(&self, id: i32) -> Result<Option<nest_rs_mcp::Json<ArticleDto>>, McpError> {
        Ok(rows()
            .into_iter()
            .find(|row| row.id == Some(id))
            .map(nest_rs_mcp::Json))
    }

    /// A prompt naming no read: what it would write in reads under no ability.
    #[prompt]
    #[public]
    async fn bare_draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            reads_under_an_ability().to_string(),
        )]))
    }

    /// A resource naming no read.
    #[resource(template = "bare-article://{id}")]
    #[public]
    async fn bare_article(&self, id: i32) -> Result<String, McpError> {
        Ok(format!("{id}:{}", reads_under_an_ability()))
    }

    /// A prompt its body writes what the caller may read into.
    #[prompt]
    #[public(Read, article::Entity, unmasked)]
    async fn draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            reads_under_an_ability().to_string(),
        )]))
    }

    /// A listing its body writes what the caller may read into.
    #[resources]
    #[public(Read, article::Entity, unmasked)]
    async fn articles(&self) -> Result<Vec<Resource>, McpError> {
        Ok(vec![Resource::new(
            "article://1",
            reads_under_an_ability().to_string(),
        )])
    }
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [Rules, AuthzGuard, ArticlesTool],
)]
struct ArticlesMcpApp;

async fn mcp_app() -> TestApp {
    TestApp::for_module::<ArticlesMcpApp>()
        .await
        .expect("the opened endpoint boots")
}

#[tokio::test]
async fn a_tool_naming_its_read_answers_a_visitor_by_its_grant() {
    let reply = call_tool(mcp_app().await.http(), MCP_PATH, "feed", None).await;
    assert!(reply.contains("article 1"), "{reply}");
    assert!(reply.contains("article 2"), "{reply}");
    assert!(
        !reply.contains("article 3"),
        "the draft is dropped: {reply}"
    );
    assert!(!reply.contains("body 1"), "the field grant holds: {reply}");
}

#[tokio::test]
async fn a_resource_naming_its_read_answers_a_visitor_by_its_grant() {
    let app = mcp_app().await;
    let client = app.http();
    let session = open_session(client, MCP_PATH, None).await;

    let published = call_method(
        client,
        MCP_PATH,
        &session,
        None,
        "resources/read",
        json!({ "uri": "article://1" }),
    )
    .await;
    assert!(published.contains("article 1"), "{published}");
    assert!(!published.contains("body 1"), "{published}");

    let draft = call_method(
        client,
        MCP_PATH,
        &session,
        None,
        "resources/read",
        json!({ "uri": "article://3" }),
    )
    .await;
    assert!(!draft.contains("article 3"), "{draft}");
}

#[tokio::test]
async fn a_prompt_and_a_listing_leaving_the_mask_to_their_body_read_under_the_visitors_rules() {
    let app = mcp_app().await;
    let client = app.http();
    let session = open_session(client, MCP_PATH, None).await;
    let prompt = call_method(
        client,
        MCP_PATH,
        &session,
        None,
        "prompts/get",
        json!({ "name": "draft" }),
    )
    .await;
    assert!(prompt.contains("\"text\":\"true\""), "{prompt}");
    let listing = call_method(
        client,
        MCP_PATH,
        &session,
        None,
        "resources/list",
        json!({}),
    )
    .await;
    assert!(listing.contains("\"name\":\"true\""), "{listing}");
}

#[tokio::test]
async fn a_bare_public_tool_reads_under_no_ability() {
    let reply = call_tool(mcp_app().await.http(), MCP_PATH, "bare", None).await;
    let answer = nest_rs_testing::mcp::result(&reply);
    assert_eq!(answer["result"]["content"][0]["text"], "false", "{reply}");
}

#[tokio::test]
async fn a_bare_public_prompt_and_resource_read_under_no_ability() {
    let app = mcp_app().await;
    let client = app.http();
    let session = open_session(client, MCP_PATH, None).await;
    let prompt = call_method(
        client,
        MCP_PATH,
        &session,
        None,
        "prompts/get",
        json!({ "name": "bare_draft" }),
    )
    .await;
    assert!(prompt.contains("\"text\":\"false\""), "{prompt}");
    let resource = call_method(
        client,
        MCP_PATH,
        &session,
        None,
        "resources/read",
        json!({ "uri": "bare-article://1" }),
    )
    .await;
    assert!(resource.contains("1:false"), "{resource}");
}

#[tokio::test]
async fn a_bare_public_tool_reading_no_entity_answers_as_before() {
    let reply = call_tool(mcp_app().await.http(), MCP_PATH, "ping", None).await;
    assert!(reply.contains("pong"), "{reply}");
}

#[tokio::test]
async fn a_tool_a_visitors_grant_does_not_reach_answers_forbidden() {
    let reply = call_tool(mcp_app().await.http(), MCP_PATH, "sealed", None).await;
    assert!(reply.contains("\"code\":\"FORBIDDEN\""), "{reply}");
    assert!(!reply.contains("reached"), "{reply}");
}
