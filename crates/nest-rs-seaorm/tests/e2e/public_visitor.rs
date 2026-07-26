//! A `#[public]` route serving real rows to an anonymous caller, end to end
//! against live Postgres: `AbilityGuard` asks the app's
//! `AbilityFactory::define_visitor`, the resulting ability scopes `Repo`'s
//! `SELECT` and masks the response. Four apps over one probe table, differing
//! only in what the visitor branch grants — nothing else about the route
//! changes, so each assertion isolates the grant.

use nest_rs_authz::http::{AbilityGuard, Authorize};
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::module;
use nest_rs_guards::guard;
use nest_rs_http::poem::web::Json;
use nest_rs_http::{controller, routes};
use nest_rs_resource::WireModelDefaults;
use nest_rs_seaorm::{CrudService, DatabaseConfig, DatabaseModule, ServiceError};
use nest_rs_testing::TestApp;
use sea_orm::DatabaseConnection;
use serde::Serialize;

mod post {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "visitor_probe_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub title: String,
        pub published: bool,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

// Stands in for what `#[expose]` emits on a real entity: `secret` carries no
// exposure, so the masker strains it out of every body regardless of the grant.
impl WireModelDefaults for post::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry(String::from("secret"))
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "title", "published"])
    }
}

// `JsonSchema` is only needed by the un-shaped route below: a route with no
// response shaper advertises its payload in the OpenAPI document.
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

/// The three rows every app in this file reads. Fixed ids and `ON CONFLICT DO
/// NOTHING`, so the nextest processes that race on the DDL converge on the same
/// data instead of each seeding its own.
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

/// One controller, reused by every app below. `#[public]` is the posture; the
/// `Authorize` parameter is the enforcement plumbing it needs — the ambient
/// ability `Repo` scopes on, and the response mask.
macro_rules! visitor_app {
    ($name:ident, $factory:ident, $define_visitor:item) => {
        mod $name {
            use super::*;

            #[nest_rs_core::injectable]
            #[derive(Default)]
            pub struct $factory;

            impl AbilityFactory for $factory {
                type Actor = ();

                fn define(&self, _actor: &(), _ab: &mut AbilityBuilder) {}

                $define_visitor
            }

            pub type VisitorGuard = AbilityGuard<$factory>;

            #[controller(path = "/posts")]
            #[use_guards(VisitorGuard)]
            pub struct PostsController;

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

                // The same route without the shaper parameter. `#[public]`
                // attaches the visitor ability to the *request*; only a shaper
                // installs it as the ambient one `Repo` reads.
                #[get("/unshaped")]
                #[public]
                async fn unshaped(&self) -> poem::Result<Json<Vec<PostDto>>> {
                    let rows = PostsService.list().await.map_err(ServiceError::from)?;
                    Ok(Json(rows.into_iter().map(PostDto::from).collect()))
                }
            }

            #[module(
                imports = [DatabaseModule::for_root(DatabaseConfig {
                    url: crate::harness::url(),
                    ..Default::default()
                })],
                providers = [$factory, VisitorGuard, PostsController],
            )]
            pub struct AppModule;

            pub async fn boot() -> TestApp {
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
    // Nothing: the default body, spelled out so the app differs from `open` in
    // exactly one rule.
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

    // The fail-closed default: `#[public]` opens the route to anonymous
    // callers, it does not grant them anything. `Authorize`'s class gate is
    // what answers — a legible 403, never an empty 200 the caller has to
    // interpret.
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

// The trap the visitor grant does *not* remove: a `#[public]` route with no
// shaper parameter never installs the ambient ability, so `Repo` fails the read
// closed on the request-scoped executor. Granting the visitor more cannot make
// this route serve a row — the fix widens what an app can declare, never what a
// mis-wired route can reach.
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
