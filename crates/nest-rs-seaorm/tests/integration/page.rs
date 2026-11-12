//! `src/page.rs` at every edge reading a cursor — a `#[crud]` list over HTTP
//! and over GraphQL, and an auto-resolved relation: a cursor no page issued is
//! refused before any query runs. The pool has no server behind it, so a
//! cursor read as the first page would answer the pool's failure instead.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::{injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_http::{EndpointGuards, HttpModule, StatusCode, controller, header};
use nest_rs_seaorm::{CrudService, SeaOrmDatabaseModule, expose};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};

use crate::always_authenticated::AlwaysAuthenticated;

const NOT_A_CURSOR: &str = "not-a-cursor";

pub(super) mod album {
    use super::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "PagedAlbum", service = super::AlbumsService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "paged_albums")]
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

    #[expose(name = "PagedTrack", service = super::TracksService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "paged_tracks")]
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

#[controller(path = "/albums")]
#[use_guards(AlwaysAuthenticated, AbilityGuard<EveryAlbum>)]
struct AlbumsController {
    #[inject]
    svc: Arc<AlbumsService>,
}

#[nest_rs_http::crud(service = AlbumsService, ops = [list])]
impl AlbumsController {}

#[resolver]
struct AlbumsResolver {
    #[inject]
    svc: Arc<AlbumsService>,
}

#[nest_rs_graphql::crud(service = AlbumsService, ops = [list])]
impl AlbumsResolver {}

/// A parent built in memory, so its relation is reached with no row read.
#[resolver]
struct ShelfResolver;

#[operations]
impl ShelfResolver {
    #[query]
    #[public]
    async fn shelved_album(&self) -> album::PagedAlbum {
        album::PagedAlbum::from(&album::Model {
            id: sea_orm::prelude::Uuid::from_u128(0x0195_b4a6_4c1c_7d3e_8f00_0000_0000_0001),
        })
    }
}

#[module(
    imports = [
        SeaOrmDatabaseModule,
        HttpModule,
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

async fn boot() -> TestApp {
    crate::over_a_pool_refusing_every_acquire()
        .await
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

/// The refusal names the parameter where the edge read it, as a value its
/// type refused is named there, and never the value.
fn assert_refuses_the_cursor(said: &str, named: &str) {
    assert_eq!(said, named);
    assert!(!said.contains(NOT_A_CURSOR), "quotes the value: {said}");
}

/// A GraphQL refusal of the cursor: async-graphql's naming of an argument its
/// type refused, under the code a malformed id answers there.
fn assert_graphql_refuses_the_cursor(error: &Value) {
    assert_refuses_the_cursor(
        error["message"].as_str().unwrap_or_default(),
        r#"Invalid value for argument "after", not a cursor this endpoint issued"#,
    );
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
    assert_refuses_the_cursor(
        problem["detail"].as_str().unwrap_or_default(),
        "query parameter `after`: not a cursor this endpoint issued",
    );
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
        &format!(r#"{{ albums(first: 2, after: "{NOT_A_CURSOR}") {{ id }} }}"#),
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
