//! `src/page.rs` at every edge reading a cursor — a `#[crud]` list over HTTP
//! and over GraphQL, and an auto-resolved relation: a cursor no page issued is
//! refused before any query runs. The pool has no server behind it, so a
//! cursor read as the first page would answer the pool's failure instead.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_http::{EndpointGuards, HttpModule, StatusCode, controller, header};
use nest_rs_seaorm::{CrudService, SeaOrmDatabaseModule, SeaOrmModule, expose};
use nest_rs_testing::TestApp;
use sea_orm::{ConnectOptions, Database};
use serde_json::{Value, json};

const NOT_A_CURSOR: &str = "not-a-cursor";

pub(super) mod album {
    use super::expose;
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[expose(name = "PagedAlbum", service = super::AlbumsService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "paged_albums",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[sea_orm(has_many)]
        #[expose]
        pub tracks: HasMany<super::track::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

pub(super) mod track {
    use super::expose;
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[expose(name = "PagedTrack", service = super::TracksService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "paged_tracks",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub album_id: Uuid,
        #[sea_orm(belongs_to, from = "album_id", to = "id")]
        #[expose]
        pub album: HasOne<super::album::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable]
#[derive(Default)]
pub(crate) struct AlbumsService;

impl CrudService for AlbumsService {
    type Entity = album::Entity;
}

#[injectable]
#[derive(Default)]
pub(crate) struct TracksService;

impl CrudService for TracksService {
    type Entity = track::Entity;
}

#[injectable]
#[derive(Default)]
struct EveryAlbum;

impl AbilityFactory for EveryAlbum {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, album::Entity);
        ab.can(Action::Read, track::Entity);
    }
}

#[injectable]
#[derive(Default)]
struct AlwaysAuthenticated;

impl Layer for AlwaysAuthenticated {}

#[async_trait]
impl Guard for AlwaysAuthenticated {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(crate::Anyone);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AlwaysAuthenticated {}

#[controller(path = "/albums")]
#[use_guards(AlwaysAuthenticated, AbilityGuard<EveryAlbum>)]
struct AlbumsController {
    #[inject]
    svc: Arc<AlbumsService>,
}

#[nest_rs_http::crud(service = svc, entity = album::Entity, output = album::PagedAlbum, ops = [list])]
impl AlbumsController {}

#[resolver]
struct AlbumsResolver {
    #[inject]
    svc: Arc<AlbumsService>,
}

#[nest_rs_graphql::crud(service = svc, entity = album::Entity, output = album::PagedAlbum, ops = [list])]
impl AlbumsResolver {}

/// A parent built in memory, so its relation is reached with no row read.
#[resolver]
struct ShelfResolver;

#[operations]
impl ShelfResolver {
    #[query]
    #[public]
    async fn shelved_album(&self) -> album::PagedAlbum {
        album::PagedAlbum {
            id: "0195b4a6-4c1c-7d3e-8f00-000000000001".to_owned(),
        }
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        HttpModule::for_root(None),
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        AlbumsService,
        TracksService,
        EveryAlbum,
        AbilityGuard<EveryAlbum>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<EveryAlbum>)>,
        AlbumsController,
        AlbumsResolver,
        ShelfResolver,
    ],
)]
struct AlbumsModule;

/// The app over a pool whose every acquire is refused within a fifth of a
/// second: nothing listens on port 1.
async fn boot() -> TestApp {
    let mut options = ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
    options
        .connect_lazy(true)
        .acquire_timeout(Duration::from_millis(200));
    let db = Database::connect(options)
        .await
        .expect("a lazy pool opens nothing");
    TestApp::builder()
        .provide(db)
        .module::<AlbumsModule>()
        .build()
        .await
        .expect("the app boots: the server's absence is a runtime fact")
}

async fn graphql(app: &TestApp, query: &str) -> Value {
    app.http()
        .post("/graphql")
        .body_json(&json!({ "query": query }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize()
}

/// The refusal names the parameter and what it expected, never the value.
fn assert_refuses_the_cursor(said: &str) {
    assert!(said.contains("after"), "names the parameter: {said}");
    assert!(!said.contains(NOT_A_CURSOR), "quotes the value: {said}");
}

/// A GraphQL refusal of the cursor: the sentence, under the code a malformed
/// id answers there.
fn assert_graphql_refuses_the_cursor(error: &Value) {
    assert_refuses_the_cursor(error["message"].as_str().unwrap_or_default());
    assert_eq!(
        error["extensions"]["code"], "INVALID_ARGUMENT",
        "the client branches on the class: {error}",
    );
}

#[tokio::test]
async fn a_list_refuses_a_cursor_it_did_not_issue_with_a_400_problem() {
    let app = boot().await;

    let refused = app
        .http()
        .get(format!("/albums?first=2&after={NOT_A_CURSOR}"))
        .send()
        .await;

    refused.assert_status(StatusCode::BAD_REQUEST);
    refused.assert_content_type("application/problem+json");
    refused.assert_header_is_not_exist(header::LINK);
    let problem: Value = refused.json().await.value().deserialize();
    assert_eq!(problem["status"], 400, "{problem}");
    assert_refuses_the_cursor(problem["detail"].as_str().unwrap_or_default());
    assert!(
        problem.get("errors").is_none(),
        "a cursor is one value, not a form: {problem}",
    );
}

#[tokio::test]
async fn a_graphql_list_refuses_a_cursor_it_did_not_issue() {
    let app = boot().await;

    let answer = graphql(
        &app,
        &format!(r#"{{ pagedAlbums(first: 2, after: "{NOT_A_CURSOR}") {{ id }} }}"#),
    )
    .await;

    assert_eq!(answer["data"], Value::Null, "no page is served: {answer}");
    let errors = answer["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(errors.len(), 1, "one refusal: {answer}");
    assert_graphql_refuses_the_cursor(&errors[0]);
}

#[tokio::test]
async fn a_relation_refuses_a_cursor_it_did_not_issue() {
    let app = boot().await;

    let answer = graphql(
        &app,
        &format!(
            r#"{{ shelvedAlbum {{ id tracks(first: 1, after: "{NOT_A_CURSOR}") {{ edges {{ cursor }} }} }} }}"#
        ),
    )
    .await;

    let errors = answer["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(errors.len(), 1, "one refusal: {answer}");
    assert_eq!(
        errors[0]["path"],
        json!(["shelvedAlbum", "tracks"]),
        "{answer}"
    );
    assert_graphql_refuses_the_cursor(&errors[0]);
}
