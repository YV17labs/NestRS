//! A `#[public]` route serving rows to an anonymous caller: the ability from
//! `AbilityFactory::define_visitor` scopes `Repo`'s `SELECT` and masks the response.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::http::Authorize;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::module;
use nest_rs_guards::guard;
use nest_rs_http::poem::web::Json;
use nest_rs_http::{controller, routes};
use nest_rs_resource::WireModelDefaults;
use nest_rs_seaorm::{CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError};
use nest_rs_testing::TestApp;
use sea_orm::DatabaseConnection;
use serde::Serialize;

mod post {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "visitor_probe_posts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub title: String,
        pub published: bool,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

// Stands in for what `#[expose]` emits: `secret` carries no exposure.
impl WireModelDefaults for post::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry(String::from("secret"))
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "title", "published"])
    }
}

// `JsonSchema`: a route with no response shaper advertises its payload in OpenAPI.
#[derive(Serialize, schemars::JsonSchema)]
struct PostDto {
    id: i32,
    title: String,
    published: bool,
}

impl From<post::Model> for PostDto {
    fn from(model: post::Model) -> Self {
        Self {
            id: model.id,
            title: model.title,
            published: model.published,
        }
    }
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
                type Actor = ();

                fn define(&self, _actor: &(), _ab: &mut AbilityBuilder) {}

                $define_visitor
            }

            pub(super) type VisitorGuard = AbilityGuard<$factory>;

            #[controller(path = "/posts")]
            #[use_guards(VisitorGuard)]
            pub(super) struct PostsController;

            #[routes]
            impl PostsController {
                #[get("/")]
                #[public]
                async fn list(
                    &self,
                    _authz: Authorize<Read, post::Entity>,
                ) -> poem::Result<Json<Vec<PostDto>>> {
                    let rows = PostsService.list().await.map_err(ServiceError::from)?;
                    Ok(Json(rows.into_iter().map(PostDto::from).collect()))
                }

                // `#[public]` attaches the ability to the request; only a shaper
                // installs it as the ambient one `Repo` reads.
                #[get("/unshaped")]
                #[public]
                async fn unshaped(&self) -> poem::Result<Json<Vec<PostDto>>> {
                    let rows = PostsService.list().await.map_err(ServiceError::from)?;
                    Ok(Json(rows.into_iter().map(PostDto::from).collect()))
                }
            }

            #[module(
                imports = [
                    SeaOrmModule::for_root(SeaOrmConfig {
                        url: crate::harness::url(),
                        ..Default::default()
                    }),
                    SeaOrmDatabaseModule,
                    ],
                providers = [$factory, VisitorGuard, PostsController],
            )]
            pub(super) struct AppModule;

            pub(super) async fn boot() -> TestApp {
                TestApp::builder()
                    .module::<AppModule>()
                    .use_guards_global([guard::<VisitorGuard>()])
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
            .fields([post::Column::Title]);
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
    resp.assert_status(poem::http::StatusCode::FORBIDDEN);
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
async fn a_field_restricted_visitor_grant_masks_the_response() {
    let _conn = seeded_db().await;
    let app = title_only::boot().await;

    let resp = app.http().get("/posts").send().await;
    resp.assert_status_is_ok();
    let rows = body_of(resp).await;
    let rows = rows.as_array().expect("a JSON array");
    assert_eq!(rows.len(), 3, "the rows themselves are all granted");
    for row in rows {
        assert!(
            row.get("title").is_some(),
            "the granted field survives: {row:?}",
        );
        assert!(
            row.get("id").is_none() && row.get("published").is_none(),
            "`.fields([Title])` masks a visitor's response exactly as it masks \
             an authenticated one: {row:?}",
        );
    }
}

// No shaper parameter, no ambient ability: `Repo` fails the read closed,
// whatever the visitor is granted.
#[tokio::test]
async fn a_public_route_without_the_shaper_still_reads_nothing() {
    let _conn = seeded_db().await;
    let app = open::boot().await;

    let resp = app.http().get("/posts/unshaped").send().await;
    resp.assert_status_is_ok();
    let rows = body_of(resp).await;
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(0),
        "no ambient ability ⇒ `Repo` denies every row: {rows:?}",
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
