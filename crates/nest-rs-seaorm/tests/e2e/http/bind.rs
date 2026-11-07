//! `src/http/bind.rs`: `#[authorize(A, bind = S)]` on a route, over the service's
//! own SQL: the row the ability admits reaches the handler, an absent one answers
//! `404` and a refused one `403`; under `Manage`, a row every concrete action admits.

use nest_rs_authz::{AbilityBuilder, Action, Manage, Read};
use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HttpView, StatusCode, controller, routes};
use nest_rs_seaorm::{Authorized, CrudService, SeaOrmDatabaseModule};
use nest_rs_testing::TestApp;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use uuid::Uuid;

use crate::harness::{ProbeMigrator, ProbeTables};

mod note {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "HttpBoundNote")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "http_bound_note")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub owner: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct NoteTables;

impl ProbeTables for NoteTables {
    fn ddl() -> String {
        "CREATE TABLE http_bound_note (id uuid PRIMARY KEY, owner text NOT NULL);".to_owned()
    }
}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

/// The ability every caller holds: its own notes alone.
#[injectable]
#[derive(Default)]
struct OwnNotes;

impl Layer for OwnNotes {}

#[async_trait]
impl Guard for OwnNotes {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let mut ability = AbilityBuilder::new();
        ability
            .can(Action::Read, note::Entity)
            .when(|p| p.eq(note::Column::Owner, "mine"));
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ability.build().expect("a valid rule set"),
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for OwnNotes {}

#[controller(path = "/notes")]
#[use_guards(OwnNotes)]
struct NotesController;

#[routes]
impl NotesController {
    #[get("/{id}")]
    #[authorize(Read, bind = NotesService)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> String {
        note.into_inner().owner
    }
}

#[module(
    imports = [SeaOrmDatabaseModule],
    providers = [OwnNotes, NotesService, NotesController],
)]
struct BoundRouteModule;

/// Every caller reads every note and changes its own alone.
#[injectable]
#[derive(Default)]
struct ManagesOwnNotes;

impl Layer for ManagesOwnNotes {}

#[async_trait]
impl Guard for ManagesOwnNotes {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let mut ability = AbilityBuilder::new();
        for action in [Action::Read, Action::Create] {
            ability.can(action, note::Entity);
        }
        for action in [Action::Update, Action::Delete] {
            ability
                .can(action, note::Entity)
                .when(|p| p.eq(note::Column::Owner, "mine"));
        }
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ability.build().expect("a valid rule set"),
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for ManagesOwnNotes {}

#[controller(path = "/managed-notes")]
#[use_guards(ManagesOwnNotes)]
struct ManagedNotesController;

#[routes]
impl ManagedNotesController {
    #[get("/{id}")]
    #[authorize(Manage, bind = NotesService)]
    async fn note(&self, note: Authorized<Manage, note::Entity>) -> String {
        note.into_inner().owner
    }
}

#[module(
    imports = [SeaOrmDatabaseModule],
    providers = [ManagesOwnNotes, NotesService, ManagedNotesController],
)]
struct ManagedRouteModule;

#[tokio::test]
async fn a_bound_route_answers_found_absent_and_denied_from_the_row() {
    let app = TestApp::builder()
        .module::<BoundRouteModule>()
        .ephemeral_database::<ProbeMigrator<NoteTables>>()
        .build()
        .await
        .expect("the route boots on its own database");
    let conn = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the app's pool is its throwaway database");
    let (mine, theirs) = (Uuid::now_v7(), Uuid::now_v7());
    conn.execute_unprepared(&format!(
        "INSERT INTO http_bound_note (id, owner) VALUES ('{mine}', 'mine'), ('{theirs}', 'theirs')"
    ))
    .await
    .expect("seed the notes");

    let found = app.http().get(format!("/notes/{mine}")).send().await;
    found.assert_status_is_ok();
    found.assert_text("mine").await;
    app.http()
        .get(format!("/notes/{}", Uuid::now_v7()))
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    app.http()
        .get(format!("/notes/{theirs}"))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

/// A note every caller reads, and only its owner changes, is refused under `Manage`.
#[tokio::test]
async fn a_route_bound_under_manage_loads_a_row_every_action_admits() {
    let app = TestApp::builder()
        .module::<ManagedRouteModule>()
        .ephemeral_database::<ProbeMigrator<NoteTables>>()
        .build()
        .await
        .expect("the route boots on its own database");
    let conn = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the app's pool is its throwaway database");
    let (mine, theirs) = (Uuid::now_v7(), Uuid::now_v7());
    conn.execute_unprepared(&format!(
        "INSERT INTO http_bound_note (id, owner) VALUES ('{mine}', 'mine'), ('{theirs}', 'theirs')"
    ))
    .await
    .expect("seed the notes");

    let found = app
        .http()
        .get(format!("/managed-notes/{mine}"))
        .send()
        .await;
    found.assert_status_is_ok();
    found.assert_text("mine").await;
    app.http()
        .get(format!("/managed-notes/{theirs}"))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    app.http()
        .get(format!("/managed-notes/{}", Uuid::now_v7()))
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}
