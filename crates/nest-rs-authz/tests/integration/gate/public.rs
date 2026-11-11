//! `#[public(Action, Subject)]`, through each edge's real decorator: what a
//! visitor reads is a line a reviewer reads, so its rows and its fields are the
//! visitor's grant's on HTTP, GraphQL, WS and MCP alike, and a bare `#[public]`
//! reads nothing a grant guards. Each handler answers the rows a `Repo` read
//! would, and says whether its reads would run under an ability: the live read
//! is `nest-rs-seaorm`'s e2e suite.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_authz::{
    Ability, AbilityBuilder, AbilityFactory, AbilityGuard, Action, Read, current_ability,
};
use nest_rs_core::{AmbientValue, Discovery, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::async_graphql::dataloader::DataLoader;
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs_graphql::{GraphqlModule, GraphqlView, dataloader, operations, resolver};
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
use serde_json::{Value, json};

use crate::graphql::query;

/// What a visitor may read: published articles, their title alone.
mod article {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Article", graphql, complex)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "articles")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub title: String,
        #[expose(nullable)]
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

use article::Article;

/// The rows a `Repo` read under the running unit's ability answers, of two
/// published articles and a draft, each judged as it is built.
fn rows() -> Vec<Article> {
    [(1, true), (2, true), (3, false)]
        .into_iter()
        .map(|(id, published)| article::Model {
            id,
            title: format!("article {id}"),
            published,
            body: Some(format!("body {id}")),
        })
        .filter(|row| {
            current_ability()
                .is_some_and(|ability| ability.can::<article::Entity>(Action::Read, row))
        })
        .map(|row| Article::from(&row))
        .collect()
}

/// The visitor's answer on an edge dropping what its grant strips.
fn titles() -> Value {
    json!([
        { "id": 1, "title": "article 1", "published": null, "body": null },
        { "id": 2, "title": "article 2", "published": null, "body": null },
    ])
}

/// A caller a token backs.
#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

/// Stands in for the authentication guard: a token attaches [`Caller`]; none
/// admits a visitor on what `#[public]` opens and at an in-band request.
#[injectable]
#[derive(Default, Layer)]
struct CallerGuard;

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
            .fields([article::Column::Id, article::Column::Title]);
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
    async fn feed(&self) -> Json<Vec<Article>> {
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

/// Says whether a batch's reads would run under an ability.
#[injectable]
#[derive(Default)]
struct Reads;

#[dataloader]
impl Reads {
    async fn under_an_ability(&self, keys: &[i32]) -> HashMap<i32, bool> {
        keys.iter()
            .map(|key| (*key, reads_under_an_ability()))
            .collect()
    }
}

/// Reads articles again by their id, as a relation's loader does: each row judged
/// where the batch builds it.
#[injectable]
#[derive(Default)]
struct Articles;

#[dataloader]
impl Articles {
    async fn by_id(&self, keys: &[i32]) -> HashMap<i32, Article> {
        keys.iter()
            .map(|&id| {
                let row = article::Model {
                    id,
                    title: format!("article {id}"),
                    published: id != 3,
                    body: Some(format!("body {id}")),
                };
                (id, Article::from(&row))
            })
            .collect()
    }
}

#[resolver]
struct ArticlesResolver;

#[operations]
impl ArticlesResolver {
    #[query]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn feed(&self) -> GqlResult<Vec<Article>> {
        Ok(rows())
    }

    #[query]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
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
    #[use_guards(AuthzGuard)]
    async fn sealed(&self) -> GqlResult<Vec<i32>> {
        Ok(Vec::new())
    }

    #[query]
    #[public(Read, Browsing)]
    #[use_guards(AuthzGuard)]
    async fn browsing(&self) -> GqlResult<bool> {
        Ok(reads_under_an_ability())
    }

    #[query]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn batched(&self, reads: &DataLoader<ReadsUnderAnAbility>) -> GqlResult<Option<bool>> {
        let Ok(read) = reads.load_one(1).await;
        Ok(read)
    }

    /// The first article, read through a batch the field itself asks.
    #[query]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn first_article(
        &self,
        articles: &DataLoader<ArticlesById>,
    ) -> GqlResult<Option<Article>> {
        let Ok(read) = articles.load_one(1).await;
        Ok(read)
    }

    /// The feed a write answers, read again.
    #[mutation]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn refreshed_feed(&self) -> GqlResult<Vec<Article>> {
        Ok(rows())
    }

    /// Each item is judged as a row: a draft dropped, a published one its title.
    #[subscription]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn feed_stream(&self) -> impl Stream<Item = Article> {
        stream::iter(rows())
    }

    /// The first article, read at the opening through a batch it asks.
    #[subscription]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn first_article_stream(
        &self,
        articles: &DataLoader<ArticlesById>,
    ) -> impl Stream<Item = Article> {
        let Ok(read) = articles.load_one(1).await;
        stream::iter(read)
    }

    /// What a stream's own body reads, after its opening's unit ended.
    #[subscription]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn stream_reads(&self) -> impl Stream<Item = bool> {
        stream::once(async { reads_under_an_ability() })
    }

    /// A field no posture reaches, below an operation naming its read.
    #[field_resolver]
    async fn nested_read(&self, parent: &Article) -> GqlResult<bool> {
        let _ = parent;
        Ok(reads_under_an_ability())
    }

    /// The article a batch reads again, as a relation reads its row.
    #[field_resolver]
    async fn reread(
        &self,
        parent: &Article,
        articles: &DataLoader<ArticlesById>,
    ) -> GqlResult<Option<Article>> {
        let Ok(read) = articles.load_one(parent.id).await;
        Ok(read)
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [
        Rules,
        CallerGuard,
        AuthzGuard,
        Reads,
        Articles,
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

/// The resolver with no guard at its `/graphql` request: each root field's own guard
/// attaches the ability, its request carrying none.
#[module(
    imports = [GraphqlModule],
    providers = [Rules, AuthzGuard, Reads, Articles, ArticlesResolver],
)]
struct OwnGuardedArticlesApp;

async fn own_guarded_app() -> TestApp {
    TestApp::for_module::<OwnGuardedArticlesApp>()
        .await
        .expect("each gated root field binds its own ability guard")
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

/// What a route's document reads: `#[public(...)]` is public and names an entity,
/// its gate answering `403` and no `401`, as the parameter it replaces did.
#[tokio::test]
async fn a_route_naming_its_read_documents_a_public_operation_on_an_entity() {
    let app = app().await;
    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let feed = controllers
        .iter()
        .flat_map(|found| found.meta.routes())
        .find(|route| route.handler() == "feed")
        .expect("the route is discovered");
    assert!(feed.public() && feed.names_an_entity());
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
async fn a_route_naming_a_capability_gates_a_visitor_and_names_no_read_of_its() {
    let app = app().await;
    let answer = app.http().get("/articles/browsing").send().await.0;
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(
        body(answer).await,
        json!(false),
        "a capability names no rows, so what a visitor reads there stays withheld",
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
async fn an_operation_answering_what_its_body_built_still_reads_under_the_visitors_rules() {
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
async fn an_operation_naming_a_capability_gates_a_visitor_and_names_no_read_of_its() {
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
        "no posture reaches a nested field, so a visitor's rules read nothing there: {visitor}",
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

/// A batch carries the request's values, never one field's, and the read a line
/// names for a visitor is that field's alone: it fails closed, so a batch answers
/// nothing, where the body of the same field reads by its grant.
#[tokio::test]
async fn a_batch_below_an_operation_naming_a_visitors_read_reads_nothing() {
    let app = app().await;
    let document = "{ feedByHand batched }";

    let visitor = query(&app, "", document).await;
    assert_eq!(
        visitor["data"],
        json!({ "feedByHand": true, "batched": false }),
        "{visitor}"
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
        read["data"],
        json!({ "feedByHand": true, "batched": true }),
        "{read}"
    );
}

/// Each item is judged for the visitor its socket carries: a draft never arrives,
/// a published article its title alone.
#[tokio::test]
async fn a_subscription_naming_its_read_judges_each_item_by_the_visitors_grant() {
    let app = app().await;
    let mut visitor = AbilityBuilder::new();
    Rules.define_visitor(&mut visitor);
    let visitor = Arc::new(visitor.build_visitor().expect("the visitor's rules build"));
    let mut socket = app.graphql_socket().ability(visitor).open();
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
    assert_eq!(done["type"], "complete", "the draft never arrived: {done}");
}

/// An ability the field's own guard attaches, the socket carrying none, judges the
/// rows its opening builds and answers them: a draft never arrives, a published
/// article its title alone.
#[tokio::test]
async fn a_subscription_whose_own_guard_attaches_its_ability_streams_its_rows() {
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
    assert_eq!(done["type"], "complete", "the draft never arrived: {done}");
}

/// A batch a subscription's opening asks judges under the ability the field's own
/// guard attached, the socket carrying its caller and no ability.
#[tokio::test]
async fn a_batch_a_subscriptions_own_guard_judges_streams_its_row() {
    let app = app().await;
    let mut socket = app.graphql_socket().attach(Arc::new(Caller)).open();
    socket.connect().await;
    socket.subscribe("1", "subscription { firstArticleStream { title } }");
    let item = socket.next_item("1").await.expect("the first article");
    assert_eq!(
        item["data"]["firstArticleStream"],
        json!({ "title": "article 1" }),
        "{item}",
    );
}

/// A subscription's opening reads the request's loaders, since its operation holds
/// one root field: a second, whose own guard attaches an ability of its own, would
/// join its batch. It is refused before either opens, and no row is judged.
#[tokio::test]
async fn a_subscription_selecting_two_own_guarded_roots_is_refused_before_either_opens() {
    let logs = LogCapture::install();
    let app = app().await;
    let mut socket = app.graphql_socket().attach(Arc::new(Caller)).open();
    socket.connect().await;
    socket.subscribe(
        "1",
        "subscription { a: firstArticleStream { title } b: firstArticleStream { title } }",
    );
    let item = socket.next_item("1").await.expect("the refusal");
    assert_eq!(item["data"], Value::Null, "{item}");
    assert_eq!(
        item["errors"][0]["message"], r#"the subscription selects "b" beside "a" at its root"#,
        "{item}",
    );
    let done = socket.next_item("1").await.expect("the operation ends");
    assert_eq!(done["type"], "complete", "{done}");
    logs.expect_none(
        "nest_rs::authz",
        "a wire row answered a caller it was not judged for",
    );
    assert!(
        logs.events_at(tracing::Level::ERROR).is_empty(),
        "{:#?}",
        logs.events(),
    );
}

/// A query whose own guard attaches the ability, its request carrying none, answers
/// the rows it built under that ability, as one its request carried would.
#[tokio::test]
async fn a_query_whose_own_guard_attaches_its_ability_answers_its_rows() {
    let answer = query(&own_guarded_app().await, "", "{ feed { title body } }").await;
    assert_eq!(
        answer,
        json!({ "data": { "feed": [
            { "title": "article 1", "body": null },
            { "title": "article 2", "body": null },
        ] } }),
    );
}

/// A mutation's reply, under the ability its own guard attached.
#[tokio::test]
async fn a_mutation_whose_own_guard_attaches_its_ability_answers_its_rows() {
    let answer = query(
        &own_guarded_app().await,
        "",
        "mutation { refreshedFeed { title } }",
    )
    .await;
    assert_eq!(
        answer,
        json!({ "data": { "refreshedFeed": [
            { "title": "article 1" },
            { "title": "article 2" },
        ] } }),
    );
}

/// A batch below such a root, a relation's, judges the rows it builds under the
/// root's ability, as one below a root its request's ability reached does.
#[tokio::test]
async fn a_batch_below_a_root_whose_own_guard_attaches_its_ability_judges_under_it() {
    let document = "{ feed { title reread { title } } }";
    let expected = json!({ "data": { "feed": [
        { "title": "article 1", "reread": { "title": "article 1" } },
        { "title": "article 2", "reread": { "title": "article 2" } },
    ] } });
    assert_eq!(
        query(&app().await, "", document).await,
        expected,
        "its request's"
    );
    assert_eq!(
        query(&own_guarded_app().await, "", document).await,
        expected
    );
}

/// A batch the root field itself asks judges under the ability its own guard
/// attached, as one under its request's does.
#[tokio::test]
async fn a_batch_a_root_whose_own_guard_attaches_its_ability_asks_judges_under_it() {
    let document = "{ firstArticle { title } }";
    let expected = json!({ "data": { "firstArticle": { "title": "article 1" } } });
    assert_eq!(
        query(&app().await, "", document).await,
        expected,
        "its request's"
    );
    assert_eq!(
        query(&own_guarded_app().await, "", document).await,
        expected
    );
}

/// A field resolver below such a root reads its parent, judged for its caller, and
/// a visitor's reads there stay closed, as below a root its request's ability reached.
#[tokio::test]
async fn a_field_below_a_root_whose_own_guard_attaches_its_ability_reads_its_parent() {
    let answer = query(
        &own_guarded_app().await,
        "",
        "{ feed { title nestedRead } }",
    )
    .await;
    assert_eq!(
        answer,
        json!({ "data": { "feed": [
            { "title": "article 1", "nestedRead": false },
            { "title": "article 2", "nestedRead": false },
        ] } }),
    );
}

/// What `ability`, carried by the socket's request, leaves a stream's own body to read.
async fn read_in_a_stream(ability: Ability, carried: Option<Arc<Caller>>) -> Value {
    let app = app().await;
    let socket = app.graphql_socket().ability(Arc::new(ability));
    let socket = match carried {
        Some(caller) => socket.attach(caller),
        None => socket,
    };
    let mut socket = socket.open();
    socket.connect().await;
    socket.subscribe("1", "subscription { streamReads }");
    let item = socket.next_item("1").await.expect("an item");
    item["data"]["streamReads"].clone()
}

/// An item a stream yields is judged where it is built, and what its body reads is
/// not: a visitor's reads there stay closed, and a caller a token backs reads under
/// what its request carried, a value derived from those rows judged by nothing.
#[tokio::test]
async fn a_streams_own_reads_are_closed_to_a_visitor_and_open_to_a_caller() {
    let mut visitor = AbilityBuilder::new();
    visitor.can(Action::Read, article::Entity);
    let visitor = visitor.build_visitor().expect("a valid rule set");
    assert_eq!(read_in_a_stream(visitor, None).await, json!(false));

    let mut caller = AbilityBuilder::new();
    caller.can(Action::Read, article::Entity);
    let caller = caller.build().expect("a valid rule set");
    assert_eq!(
        read_in_a_stream(caller, Some(Arc::new(Caller))).await,
        json!(true),
    );
}

/// What a stream's own body reads under an ability its field's own guard attached,
/// the socket carrying none: what it would under one the socket carried — a caller
/// a token backs reads under it, a visitor nothing.
#[tokio::test]
async fn an_ability_a_fields_own_guard_attaches_reaches_its_streams_reads_as_a_carried_one() {
    let app = app().await;
    for (caller, reads) in [(Some(Arc::new(Caller)), true), (None, false)] {
        let socket = app.graphql_socket();
        let socket = match caller {
            Some(caller) => socket.attach(caller),
            None => socket,
        };
        let mut socket = socket.open();
        socket.connect().await;
        socket.subscribe("1", "subscription { streamReads }");
        let item = socket.next_item("1").await.expect("an item");
        assert_eq!(item["data"]["streamReads"], reads, "{item}");
    }
}

#[gateway(path = "/ws/articles")]
#[derive(Default)]
struct ArticlesGateway;

#[messages]
impl ArticlesGateway {
    #[subscribe_message("feed")]
    #[public(Read, article::Entity)]
    #[use_guards(AuthzGuard)]
    async fn feed(&self) -> Result<Vec<Article>, WsError> {
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
async fn a_message_dispatched_on_its_own_judges_by_the_visitors_grant_too() {
    let mut visitor = AbilityBuilder::new();
    visitor
        .can(Action::Read, article::Entity)
        .when(|p| p.eq(article::Column::Published, true))
        .fields([article::Column::Id, article::Column::Title]);
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
async fn a_message_naming_a_capability_gates_a_visitor_and_names_no_read_of_its() {
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
    async fn feed(&self) -> Result<nest_rs_mcp::Json<Vec<Article>>, McpError> {
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
    async fn article(&self, id: i32) -> Result<Option<nest_rs_mcp::Json<Article>>, McpError> {
        Ok(rows()
            .into_iter()
            .find(|row| row.id == id)
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
    #[public(Read, article::Entity)]
    async fn draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            reads_under_an_ability().to_string(),
        )]))
    }

    /// A listing its body writes what the caller may read into.
    #[resources]
    #[public(Read, article::Entity)]
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
async fn a_prompt_and_a_listing_answering_what_their_body_built_read_under_the_visitors_rules() {
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
