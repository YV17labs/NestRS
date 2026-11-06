//! `src/service.rs` in process: a `#[crud]` create a grant's `.fields` refuses
//! answers `403` on HTTP and GraphQL, naming the input's field by its client's
//! spelling, on a pool whose every acquire is refused, so an attempt at a statement
//! would answer its `500`; and a service's `?` on the same refusal.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::{code, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, resolver};
use nest_rs_http::{EndpointGuards, StatusCode, controller};
use nest_rs_seaorm::{Creatable, CrudService, SeaOrmDatabaseModule, ServiceError, expose};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::{ConnectOptions, Database, Set};
use serde_json::{Value, json};

use crate::always_authenticated::AlwaysAuthenticated;

mod note {
    use super::*;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub title: String,
        #[sea_orm(column_name = "review_state")]
        #[expose(input(create))]
        pub status: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

impl Creatable for NotesService {
    type Create = note::CreateNote;
}

impl NotesService {
    /// A body naming a column of its own, past `#[crud]`'s input.
    async fn stamp(&self) -> Result<(), ServiceError> {
        let active = note::ActiveModel {
            id: Set(sea_orm::prelude::Uuid::now_v7()),
            status: Set("reviewed".to_owned()),
            ..Default::default()
        };
        self.create_from_active(active).await?;
        Ok(())
    }
}

/// Writes the title of a note, and nothing else of one.
#[injectable]
#[derive(Default)]
struct TitlesOnly;

impl AbilityFactory for TitlesOnly {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, note::Entity);
        ab.can(Action::Create, note::Entity)
            .fields([note::Column::Title]);
    }
}

#[controller(path = "/notes")]
#[use_guards(AlwaysAuthenticated, AbilityGuard<TitlesOnly>)]
struct NotesController {
    #[inject]
    svc: Arc<NotesService>,
}

#[nest_rs_http::crud(service = NotesService, ops = [create])]
impl NotesController {}

#[controller(path = "/stamps")]
#[use_guards(AlwaysAuthenticated, AbilityGuard<TitlesOnly>)]
struct StampsController {
    #[inject]
    svc: Arc<NotesService>,
}

#[nest_rs_http::routes]
impl StampsController {
    #[post("/")]
    #[authorize(nest_rs_authz::Create, note::Entity)]
    async fn stamp(&self) -> Result<(), ServiceError> {
        self.svc.stamp().await
    }
}

#[resolver]
struct NotesResolver {
    #[inject]
    svc: Arc<NotesService>,
}

#[nest_rs_graphql::crud(service = NotesService, ops = [list, create])]
impl NotesResolver {}

#[module(
    imports = [
        SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        NotesService,
        TitlesOnly,
        AbilityGuard<TitlesOnly>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<TitlesOnly>)>,
        NotesController,
        StampsController,
        NotesResolver,
    ],
)]
struct NotesModule;

/// The app over a pool whose every acquire is refused: nothing listens on port 1.
async fn boot() -> TestApp {
    let mut options = ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
    options
        .connect_lazy(true)
        .acquire_timeout(Duration::from_millis(50));
    let db = Database::connect(options)
        .await
        .expect("a lazy pool opens nothing");
    TestApp::builder()
        .provide(db)
        .module::<NotesModule>()
        .build()
        .await
        .expect("the app boots: the server's absence is a runtime fact")
}

#[tokio::test]
async fn a_crud_create_on_http_names_the_field_its_grant_withholds() {
    let logs = LogCapture::install();
    let app = boot().await;

    let res = app
        .http()
        .post("/notes")
        .body_json(&json!({ "title": "minutes", "status": "secret-value" }))
        .send()
        .await;

    res.assert_status(StatusCode::FORBIDDEN);
    let body: Value = res.json().await.value().deserialize();
    assert_eq!(body["code"], code::FORBIDDEN.as_str(), "{body}");
    assert_eq!(
        body["errors"],
        json!({ "status": [{ "code": "forbidden" }] }),
        "the field as the client spells it, never the column's name: {body}",
    );
    let said = body.to_string();
    assert!(!said.contains("review_state"), "{said}");
    assert!(!said.contains("secret-value"), "{said}");

    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "access denied — a column the caller's grants withhold",
    );
    assert_eq!(event.level, "warn");
    assert!(!event.quotes("secret-value"), "{event:#?}");
}

#[tokio::test]
async fn a_crud_create_on_graphql_names_the_field_its_grant_withholds() {
    let app = boot().await;

    let body: Value = app
        .http()
        .post("/graphql")
        .body_json(&json!({
            "query": r#"mutation { createNote(input: { title: "minutes", status: "secret-value" }) { id } }"#,
        }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();

    let error = &body["errors"][0];
    assert_eq!(error["message"], "forbidden", "{body}");
    assert_eq!(
        error["extensions"]["code"],
        code::FORBIDDEN.as_str(),
        "{body}"
    );
    assert_eq!(
        error["extensions"]["errors"],
        json!({ "status": [{ "code": "forbidden" }] }),
        "{body}",
    );
    assert!(!body.to_string().contains("secret-value"), "{body}");
}

/// A body answering `ServiceError` reads the same class, its `?` the whole
/// conversion, and names the column as the table does: no input names it for it.
#[tokio::test]
async fn a_services_write_answers_forbidden_naming_the_column() {
    let app = boot().await;

    let res = app.http().post("/stamps").send().await;

    res.assert_status(StatusCode::FORBIDDEN);
    let body: Value = res.json().await.value().deserialize();
    assert_eq!(body["code"], code::FORBIDDEN.as_str(), "{body}");
    assert_eq!(
        body["detail"], "the caller may not write review_state",
        "{body}"
    );
}
