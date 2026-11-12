//! A `#[public]` route serving rows to an anonymous caller: the ability from
//! `AbilityFactory::define_visitor` scopes `Repo`'s `SELECT` and judges each wire
//! row the route answers.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::module;
use nest_rs_guards::GlobalGuards;
use nest_rs_http::Json;
use nest_rs_http::{controller, routes};
use nest_rs_seaorm::{CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::DatabaseConnection;

mod post {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "VisitorPost", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "visitor_probe_posts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: i32,
        #[expose]
        pub title: String,
        #[expose(nullable)]
        pub published: bool,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct PostsService;

impl CrudService for PostsService {
    type Entity = post::Entity;
}

/// The three rows every app in this file reads, seeded idempotently.
async fn seeded_db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "visitor_probe_posts",
        "CREATE TABLE IF NOT EXISTS visitor_probe_posts (
            id INT PRIMARY KEY,
            title TEXT NOT NULL,
            published BOOLEAN NOT NULL,
            secret TEXT NOT NULL
        );
         INSERT INTO visitor_probe_posts (id, title, published, secret) VALUES
            (1, 'published one', true,  'sauce-1'),
            (2, 'published two', true,  'sauce-2'),
            (3, 'a draft',       false, 'sauce-3')
         ON CONFLICT (id) DO NOTHING;",
    )
    .await;
    conn
}

/// One app per visitor grant, over the same controller.
macro_rules! visitor_app {
    ($name:ident, $factory:ident, $define_visitor:item) => {
        mod $name {
            use super::*;

            #[nest_rs_core::injectable]
            #[derive(Default)]
            pub(super) struct $factory;

            impl AbilityFactory for $factory {
                type Actor = crate::Anyone;

                fn define(&self, _actor: &crate::Anyone, _ab: &mut AbilityBuilder) {}

                $define_visitor
            }

            pub(super) type VisitorGuard = AbilityGuard<$factory>;

            #[controller(path = "/posts")]
            #[use_guards(VisitorGuard)]
            pub(super) struct PostsController;

            #[routes]
            impl PostsController {
                #[get("/")]
                #[public(Read, post::Entity)]
                async fn list(&self) -> nest_rs_http::Result<Json<Vec<post::VisitorPost>>> {
                    let rows = PostsService.list().await.map_err(ServiceError::from)?;
                    Ok(Json(rows.iter().map(post::VisitorPost::from).collect()))
                }

                // The bare word names no read: the ability the guard attached is
                // withheld from what `Repo` reads.
                #[get("/bare")]
                #[public]
                async fn bare(&self) -> nest_rs_http::Result<Json<Vec<post::VisitorPost>>> {
                    let rows = PostsService.list().await.map_err(ServiceError::from)?;
                    Ok(Json(rows.iter().map(post::VisitorPost::from).collect()))
                }
            }

            #[module(
                imports = [
                    SeaOrmModule::for_root(SeaOrmConfig {
                        url: crate::harness::url().into(),
                        ..Default::default()
                    }),
                    SeaOrmDatabaseModule,
                    ],
                providers = [$factory, VisitorGuard, PostsController, GlobalGuards<(VisitorGuard,)>],
            )]
            pub(super) struct AppModule;

            pub(super) async fn boot() -> TestApp {
                TestApp::builder()
                    .module::<AppModule>()
                    .build()
                    .await
                    .expect("the visitor app boots against live Postgres")
            }
        }
    };
}

visitor_app!(
    open,
    OpenToVisitors,
    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, post::Entity);
    }
);

visitor_app!(
    closed,
    ClosedToVisitors,
    // The default body, spelled out so the app differs from `open` in one rule.
    fn define_visitor(&self, _ab: &mut AbilityBuilder) {}
);

visitor_app!(
    published_only,
    PublishedToVisitors,
    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::Published, true));
    }
);

visitor_app!(
    title_only,
    TitleToVisitors,
    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, post::Entity)
            .fields([post::Column::Id, post::Column::Title]);
    }
);

async fn body_of(resp: nest_rs_testing::TestResponse) -> serde_json::Value {
    let body = resp.0.into_body().into_string().await.expect("body");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("json body ({err}): {body}"))
}

#[tokio::test]
async fn a_visitor_grant_serves_rows_to_an_anonymous_caller() {
    let _conn = seeded_db().await;
    let app = open::boot().await;

    let resp = app.http().get("/posts").send().await;
    resp.assert_status_is_ok();
    let rows = body_of(resp).await;
    let rows = rows.as_array().expect("a JSON array");
    assert_eq!(
        rows.len(),
        3,
        "an unconditional visitor grant serves every row: {rows:?}",
    );
}

#[tokio::test]
async fn without_a_visitor_grant_a_public_route_is_forbidden() {
    let _conn = seeded_db().await;
    let app = closed::boot().await;

    // `#[public]` opens the route, it grants nothing.
    let resp = app.http().get("/posts").send().await;
    resp.assert_status(nest_rs_http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_conditional_visitor_grant_scopes_the_query() {
    let _conn = seeded_db().await;
    let app = published_only::boot().await;

    let resp = app.http().get("/posts").send().await;
    resp.assert_status_is_ok();
    let rows = body_of(resp).await;
    let rows = rows.as_array().expect("a JSON array");
    assert_eq!(rows.len(), 2, "only published rows survive: {rows:?}");
    assert!(
        rows.iter()
            .all(|r| r["published"] == serde_json::json!(true)),
        "the `.when(...)` predicate reached the SQL: {rows:?}",
    );
}

#[tokio::test]
async fn a_field_restricted_visitor_grant_withholds_the_column_it_leaves_out() {
    let _conn = seeded_db().await;
    let app = title_only::boot().await;

    let resp = app.http().get("/posts").send().await;
    resp.assert_status_is_ok();
    let rows = body_of(resp).await;
    let rows = rows.as_array().expect("a JSON array");
    assert_eq!(rows.len(), 3, "the rows themselves are all granted");
    for row in rows {
        assert!(
            row["id"].is_i64() && row["title"].is_string(),
            "the granted fields survive: {row:?}",
        );
        assert_eq!(
            row.get("published"),
            Some(&serde_json::Value::Null),
            "`.fields([Id, Title])` withholds a visitor's column exactly as it \
             withholds an authenticated caller's, its key written: {row:?}",
        );
    }
}

// No read named: `Repo` fails it closed, whatever the visitor is granted, and
// its `warn` names the remedy.
#[tokio::test]
async fn a_bare_public_route_reads_nothing_and_says_what_names_the_read() {
    let logs = LogCapture::install();
    let _conn = seeded_db().await;
    let app = open::boot().await;

    let resp = app.http().get("/posts/bare").send().await;
    resp.assert_status_is_ok();
    let rows = body_of(resp).await;
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(0),
        "no ambient ability ⇒ `Repo` denies every row: {rows:?}",
    );
    let event = logs.expect_one(
        "nest_rs::orm",
        "no ambient Ability outside a worker job — denying all rows",
    );
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("hint")
            .is_some_and(|hint| hint.contains("#[public(Read, Entity)]")),
        "{:?}",
        event.fields,
    );
}

#[tokio::test]
async fn the_unexposed_column_never_reaches_a_visitor() {
    let _conn = seeded_db().await;
    let app = open::boot().await;

    let resp = app.http().get("/posts").send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.expect("body");
    assert!(
        !body.contains("secret") && !body.contains("sauce-"),
        "an unrestricted visitor grant still cannot leak an unexposed column: {body}",
    );
}

/// The same read on every edge, against live rows: what `#[public(Read, Entity)]`
/// names answers the published rows and the granted field alone, and a bare
/// `#[public]` reading them answers nothing, with the `warn` naming that line.
mod every_edge {
    use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Read};
    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
    use nest_rs_http::{EndpointGuards, Json, StatusCode, controller, routes};
    use nest_rs_mcp::{McpAccess, McpModule, mcp, tools};
    use nest_rs_seaorm::{
        Authorized, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError,
    };
    use nest_rs_testing::mcp::call_tool;
    use nest_rs_testing::{LogCapture, TestApp};
    use nest_rs_ws::{WsModule, gateway, messages};
    use serde_json::{Value, json};

    use super::{PostsService, post, seeded_db};

    /// What every edge answers: the wire rows of what the visitor may read, each
    /// judged where it is built.
    async fn feed() -> Result<Vec<post::VisitorPost>, ServiceError> {
        let rows = PostsService.list().await.map_err(ServiceError::from)?;
        Ok(rows.iter().map(post::VisitorPost::from).collect())
    }

    /// A post a route binds by id, its key a UUID v7.
    mod bound {
        use nest_rs_seaorm::expose;
        use sea_orm::entity::prelude::*;

        #[expose(name = "BoundRow")]
        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "visitor_bound_posts")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            #[expose]
            pub id: Uuid,
            #[expose]
            pub title: String,
            #[expose(nullable)]
            pub published: bool,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    #[injectable]
    #[derive(Default)]
    struct BoundService;

    impl CrudService for BoundService {
        type Entity = bound::Entity;
    }

    const PUBLISHED: &str = "0195b4a6-4c1c-7d3e-8f00-000000000001";
    const DRAFT: &str = "0195b4a6-4c1c-7d3e-8f00-000000000002";
    const ABSENT: &str = "0195b4a6-4c1c-7d3e-8f00-000000000003";

    async fn seed_bound_rows() {
        let conn = crate::harness::connect().await;
        crate::harness::setup_shared_table(
            &conn,
            "visitor_bound_posts",
            &format!(
                "CREATE TABLE IF NOT EXISTS visitor_bound_posts (
                    id UUID PRIMARY KEY,
                    title TEXT NOT NULL,
                    published BOOLEAN NOT NULL
                );
                 INSERT INTO visitor_bound_posts (id, title, published) VALUES
                    ('{PUBLISHED}', 'published', true),
                    ('{DRAFT}', 'a draft', false)
                 ON CONFLICT (id) DO NOTHING;"
            ),
        )
        .await;
    }

    #[injectable]
    #[derive(Default)]
    struct Rules;

    impl AbilityFactory for Rules {
        type Actor = crate::Anyone;

        fn define(&self, _actor: &crate::Anyone, _ab: &mut AbilityBuilder) {}

        fn define_visitor(&self, ab: &mut AbilityBuilder) {
            ab.can(Action::Read, post::Entity)
                .when(|p| p.eq(post::Column::Published, true))
                .fields([post::Column::Id, post::Column::Title]);
            ab.can(Action::Read, bound::Entity)
                .when(|p| p.eq(bound::Column::Published, true))
                .fields([bound::Column::Id, bound::Column::Title]);
        }
    }

    type VisitorGuard = AbilityGuard<Rules>;

    /// The one `warn` a read no ability reaches files, naming the line to write.
    fn assert_the_read_said_so(logs: &LogCapture) {
        let event = logs.expect_one(
            "nest_rs::orm",
            "no ambient Ability outside a worker job — denying all rows",
        );
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("hint")
                .is_some_and(|hint| hint.contains("#[public(Read, Entity)]")),
            "{:?}",
            event.fields,
        );
    }

    /// The rows a visitor's grant serves, in any order: each its key and its
    /// title, and `published` withheld.
    fn assert_the_published_titles_alone(rows: &Value) {
        let rows = rows.as_array().expect("a list of rows");
        let mut titles: Vec<&str> = rows
            .iter()
            .filter_map(|row| row["title"].as_str())
            .collect();
        titles.sort_unstable();
        assert_eq!(titles, ["published one", "published two"], "{rows:?}");
        for row in rows {
            assert!(row["id"].is_i64(), "{row}");
            assert_eq!(row.get("published"), Some(&Value::Null), "{row}");
        }
    }

    #[controller(path = "/edge/posts")]
    #[use_guards(VisitorGuard)]
    struct PostsController;

    #[routes]
    impl PostsController {
        #[get("/")]
        #[public(Read, post::Entity)]
        async fn named(&self) -> nest_rs_http::Result<Json<Vec<post::VisitorPost>>> {
            Ok(Json(feed().await?))
        }

        #[get("/one/{id}")]
        #[public(Read, bind = BoundService)]
        async fn one(
            &self,
            row: Authorized<Read, bound::Entity>,
        ) -> nest_rs_http::Result<Json<bound::BoundRow>> {
            Ok(Json(bound::BoundRow::from(&row.into_inner())))
        }
    }

    #[resolver]
    struct PostsResolver;

    #[operations]
    impl PostsResolver {
        #[query]
        #[public(Read, post::Entity)]
        async fn named(&self) -> Result<Vec<post::VisitorPost>, ServiceError> {
            feed().await
        }

        #[query]
        #[public]
        async fn bare(&self) -> Result<Vec<post::VisitorPost>, ServiceError> {
            feed().await
        }
    }

    #[module(
        imports = [
            SeaOrmModule::for_root(SeaOrmConfig {
                url: crate::harness::url().into(),
                ..Default::default()
            }),
            SeaOrmDatabaseModule,
            GraphqlModule,
        ],
        providers = [
            Rules,
            VisitorGuard,
            BoundService,
            PostsController,
            PostsResolver,
            EndpointGuards<GraphqlView, (VisitorGuard,)>,
        ],
    )]
    struct HttpAndGraphqlApp;

    async fn http_and_graphql() -> TestApp {
        TestApp::builder()
            .module::<HttpAndGraphqlApp>()
            .build()
            .await
            .expect("the routes and the resolver boot against live Postgres")
    }

    async fn graphql(app: &TestApp, document: &str) -> Value {
        let answer = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": document }))
            .send()
            .await
            .json()
            .await;
        serde_json::to_value(answer).expect("a GraphQL response")
    }

    #[tokio::test]
    async fn a_route_naming_its_read_answers_the_published_rows_and_the_granted_field() {
        let _conn = seeded_db().await;
        let app = http_and_graphql().await;

        let answer = app.http().get("/edge/posts").send().await;
        answer.assert_status_is_ok();
        assert_the_published_titles_alone(&super::body_of(answer).await);
    }

    #[tokio::test]
    async fn a_public_route_bound_on_its_id_answers_by_the_visitors_grant() {
        seed_bound_rows().await;
        let app = http_and_graphql().await;

        let published = app
            .http()
            .get(format!("/edge/posts/one/{PUBLISHED}"))
            .send()
            .await;
        published.assert_status_is_ok();
        assert_eq!(
            super::body_of(published).await,
            json!({ "id": PUBLISHED, "title": "published", "published": null })
        );

        for (id, status) in [
            (DRAFT, StatusCode::FORBIDDEN),
            (ABSENT, StatusCode::NOT_FOUND),
        ] {
            app.http()
                .get(format!("/edge/posts/one/{id}"))
                .send()
                .await
                .assert_status(status);
        }
    }

    #[tokio::test]
    async fn an_operation_naming_its_read_answers_the_published_rows_and_the_granted_field() {
        let _conn = seeded_db().await;
        let app = http_and_graphql().await;

        let answer = graphql(&app, "{ named { title id published } }").await;
        let rows = answer["data"]["named"].as_array().expect("rows");
        assert_eq!(rows.len(), 2, "{answer}");
        for row in rows {
            assert!(row["title"].is_string() && row["id"].is_i64(), "{row}");
            assert!(row["published"].is_null(), "{row}");
        }
    }

    #[tokio::test]
    async fn a_bare_public_operation_reads_nothing_and_says_what_names_the_read() {
        let logs = LogCapture::install();
        let _conn = seeded_db().await;
        let app = http_and_graphql().await;

        let answer = graphql(&app, "{ bare { title } }").await;
        assert_eq!(answer["data"]["bare"], json!([]), "{answer}");
        assert_the_read_said_so(&logs);
    }

    #[gateway(path = "/edge/ws")]
    struct PostsGateway;

    #[messages]
    impl PostsGateway {
        #[subscribe_message("named")]
        #[public(Read, post::Entity)]
        #[use_guards(VisitorGuard)]
        async fn named(&self) -> Result<Vec<post::VisitorPost>, ServiceError> {
            feed().await
        }

        #[subscribe_message("bare")]
        #[public]
        #[use_guards(VisitorGuard)]
        async fn bare(&self) -> Result<Vec<post::VisitorPost>, ServiceError> {
            feed().await
        }
    }

    #[module(
        imports = [
            SeaOrmModule::for_root(SeaOrmConfig {
                url: crate::harness::url().into(),
                ..Default::default()
            }),
            SeaOrmDatabaseModule,
            WsModule,
        ],
        providers = [Rules, VisitorGuard, PostsGateway],
    )]
    struct WsApp;

    /// What the gateway answers `event` over a real upgrade.
    async fn message(event: &str) -> Value {
        let app = TestApp::builder()
            .module::<WsApp>()
            .build_ws()
            .await
            .expect("the gateway boots against live Postgres");
        let mut socket = app.socket("/edge/ws").connect().await;
        socket.send(event, Value::Null).await;
        let frame = socket.next_envelope().await;
        app.shutdown().await.expect("the transport stops cleanly");
        frame
    }

    #[tokio::test]
    async fn a_message_naming_its_read_answers_the_published_rows_and_the_granted_field() {
        let _conn = seeded_db().await;
        assert_the_published_titles_alone(&message("named").await["data"]);
    }

    #[tokio::test]
    async fn a_bare_public_message_reads_nothing_and_says_what_names_the_read() {
        let logs = LogCapture::install();
        let _conn = seeded_db().await;
        assert_eq!(message("bare").await["data"], json!([]));
        assert_the_read_said_so(&logs);
    }

    const MCP_PATH: &str = "/edge/mcp";

    #[mcp(path = "/edge/mcp")]
    #[use_guards(VisitorGuard)]
    #[derive(Clone, Default)]
    struct PostsTool;

    #[tools]
    impl PostsTool {
        /// What a visitor may read.
        #[tool]
        #[public(Read, post::Entity)]
        async fn named(&self) -> Result<nest_rs_mcp::Json<Vec<post::VisitorPost>>, ServiceError> {
            feed().await.map(nest_rs_mcp::Json)
        }

        /// The same read, naming nothing.
        #[tool]
        #[public]
        async fn bare(&self) -> Result<nest_rs_mcp::Json<Vec<post::VisitorPost>>, ServiceError> {
            feed().await.map(nest_rs_mcp::Json)
        }
    }

    #[module(
        imports = [
            SeaOrmModule::for_root(SeaOrmConfig {
                url: crate::harness::url().into(),
                ..Default::default()
            }),
            SeaOrmDatabaseModule,
            McpModule::for_root(McpAccess::Public),
        ],
        providers = [Rules, VisitorGuard, PostsTool],
    )]
    struct McpApp;

    async fn tool(name: &str) -> String {
        let app = TestApp::for_module::<McpApp>()
            .await
            .expect("the opened endpoint boots against live Postgres");
        call_tool(app.http(), MCP_PATH, name, None).await
    }

    #[tokio::test]
    async fn a_tool_naming_its_read_answers_the_published_rows_and_the_granted_field() {
        let _conn = seeded_db().await;
        let reply = tool("named").await;
        assert!(reply.contains("published one"), "{reply}");
        assert!(reply.contains("published two"), "{reply}");
        assert!(!reply.contains("a draft"), "{reply}");
        assert!(
            !reply.contains("published\\\":true") && !reply.contains("\"published\":true"),
            "the field grant holds: {reply}"
        );
        assert!(
            reply.contains("published\\\":null") || reply.contains("\"published\":null"),
            "the withheld column's key is written: {reply}"
        );
    }

    #[tokio::test]
    async fn a_bare_public_tool_reads_nothing_and_says_what_names_the_read() {
        let logs = LogCapture::install();
        let _conn = seeded_db().await;
        let reply = tool("bare").await;
        assert!(!reply.contains("published one"), "{reply}");
        assert_the_read_said_so(&logs);
    }
}
