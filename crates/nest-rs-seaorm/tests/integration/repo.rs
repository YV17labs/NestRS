//! `src/repo.rs`'s saves and `src/service.rs`'s create under a route gated on a
//! `#[subject]`: the unit reads under the ability its guard attached, which grants
//! the capability and nothing on the entity, so no row could pass their scope.
//! They answer that before the entity's `before_save` runs or a statement is sent,
//! on a pool that dials nothing, where a statement sent would answer its connection
//! error. A server's refusal of a statement `Repo` sent is handed out said by its
//! SQLSTATE and the names the server reported, never the row it quoted.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Manage, subject};
use nest_rs_core::{injectable, module};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec};
use nest_rs_http::{Json, StatusCode, controller, routes};
use nest_rs_seaorm::{
    Creatable, CreateModel, CrudService, Repo, RepoError, SeaOrmConfig, SeaOrmDatabaseModule,
    SeaOrmUnitOfWork,
};
use sea_orm::{IntoActiveModel, Set};
use serde_json::{Value, json};

use crate::always_authenticated::AlwaysAuthenticated;

/// How many saves reached the entity's `before_save`.
static SAVES: AtomicUsize = AtomicUsize::new(0);

mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub body: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    #[async_trait::async_trait]
    impl ActiveModelBehavior for ActiveModel {
        async fn before_save<C: ConnectionTrait>(self, _: &C, _: bool) -> Result<Self, DbErr> {
            super::SAVES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self)
        }
    }
}

struct CreateNote;

impl CreateModel<note::Entity> for CreateNote {
    fn into_active_model(self) -> note::ActiveModel {
        note::ActiveModel {
            id: Set(uuid::Uuid::now_v7()),
            body: Set("hello".to_owned()),
        }
    }
}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

impl Creatable for NotesService {
    type Create = CreateNote;
}

/// May run the org's reports.
#[subject]
struct Reporting;

#[injectable]
#[derive(Default)]
struct Reporter;

impl AbilityFactory for Reporter {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Manage, Reporting);
    }
}

#[controller(path = "/reports")]
#[use_guards(AlwaysAuthenticated, AbilityGuard<Reporter>)]
struct ReportsController {
    #[inject]
    svc: Arc<NotesService>,
}

#[routes]
impl ReportsController {
    #[post("/saved")]
    #[authorize(Manage, Reporting)]
    async fn saved(&self) -> nest_rs_http::Result<Json<Value>> {
        let mut row = note::Model {
            id: uuid::Uuid::now_v7(),
            body: "hello".to_owned(),
        }
        .into_active_model();
        row.body = Set("goodbye".to_owned());
        Ok(Json(reply(Repo::<note::Entity>::update(row).await)))
    }

    #[post("/created")]
    #[authorize(Manage, Reporting)]
    async fn created(&self) -> nest_rs_http::Result<Json<Value>> {
        Ok(Json(reply(self.svc.create(CreateNote).await)))
    }

    #[post("/registered")]
    #[authorize(Manage, Reporting)]
    async fn registered(&self) -> nest_rs_http::Result<Json<Value>> {
        let note = self
            .svc
            .create(CreateNote)
            .await
            .map_err(nest_rs_seaorm::ServiceError::from)?;
        Ok(Json(json!({ "id": note.id })))
    }
}

fn reply(written: Result<note::Model, nest_rs_seaorm::RepoError>) -> Value {
    json!({
        "hooks": SAVES.load(Ordering::SeqCst),
        "written": written.map(|_row| "a row").map_err(|err| format!("{err:?}")),
    })
}

#[module(
    imports = [SeaOrmDatabaseModule],
    providers = [
        NotesService,
        Reporter,
        AbilityGuard<Reporter>,
        AlwaysAuthenticated,
        ReportsController,
    ],
)]
struct ReportsModule;

async fn answered(path: &str) -> Value {
    let app = crate::over_an_unconnected_pool()
        .module::<ReportsModule>()
        .build()
        .await
        .expect("the app boots over an unconnected pool");
    let resp = app.http().post(path).send().await;
    resp.assert_status(StatusCode::OK);
    resp.json().await.value().deserialize::<Value>()
}

#[tokio::test]
async fn a_save_on_a_route_gated_on_a_subject_runs_no_hook_and_sends_nothing() {
    assert_eq!(
        answered("/reports/saved").await,
        json!({ "hooks": 0, "written": { "Err": "NotFound" } }),
    );
}

/// A create its scope refuses answers through a service's `?` the `403` a
/// `#[crud]` create answers, never an opaque `500`.
#[tokio::test]
async fn a_create_a_service_refuses_for_its_scope_answers_forbidden() {
    let app = crate::over_an_unconnected_pool()
        .module::<ReportsModule>()
        .build()
        .await
        .expect("the app boots over an unconnected pool");
    let resp = app.http().post("/reports/registered").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    let body = resp.json().await.value().deserialize::<Value>();
    assert_eq!(body["code"], "FORBIDDEN", "{body}");
    assert_eq!(SAVES.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_create_on_a_route_gated_on_a_subject_runs_no_hook_and_sends_nothing() {
    assert_eq!(
        answered("/reports/created").await,
        json!({ "hooks": 0, "written": { "Err": "Forbidden { columns: [] }" } }),
    );
}

/// The row value the double's refusals quote, as Postgres quotes the row it refused.
const MARKED: &str = "zzqx-ada@example.com";

/// What Postgres answers an insert meeting a unique key with: the key's value in
/// its `detail`.
const UNIQUE_VIOLATION: [(u8, &str); 9] = [
    (b'S', "ERROR"),
    (b'V', "ERROR"),
    (b'C', "23505"),
    (
        b'M',
        "duplicate key value violates unique constraint \"notes_body_key\"",
    ),
    (b'D', "Key (body)=(zzqx-ada@example.com) already exists."),
    (b's', "public"),
    (b't', "notes"),
    (b'n', "notes_body_key"),
    (b'R', "_bt_check_unique"),
];

/// What Postgres answers a value its column's type refuses with: the value in its
/// message.
const INVALID_TEXT: [(u8, &str); 5] = [
    (b'S', "ERROR"),
    (b'V', "ERROR"),
    (b'C', "22P02"),
    (
        b'M',
        "invalid input syntax for type uuid: \"zzqx-ada@example.com\"",
    ),
    (b'R', "string_to_uuid"),
];

/// `body` in a pool unit of the binding over a pool on `url`, as a job runs.
async fn in_a_job_on<T: Send>(url: &str, body: impl Future<Output = T> + Send) -> T {
    let mut options = sea_orm::ConnectOptions::new(url);
    options.connect_lazy(true).sqlx_logging(false);
    let db = sea_orm::Database::connect(options)
        .await
        .expect("a lazy pool dials nothing");
    let binding: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(
        Arc::new(db),
        &SeaOrmConfig::default(),
    ));
    nest_rs_database::run_unit(
        Some(&binding),
        UnitSpec::new(
            nest_rs_core::unit::HOOK,
            ExecutorScope::Job,
            TransactionPolicy::Pool,
        ),
        body,
        |_| true,
        |why| panic!("a pool unit is always opened: {why}"),
    )
    .await
}

/// A unique violation an insert meets is handed out as the variant a service
/// branches on, naming its key; no reader of it prints the `detail` quoting the row.
#[tokio::test]
async fn a_write_the_server_refuses_is_handed_out_naming_no_value_it_quoted() {
    let (url, _root) = crate::postgres_refusing_every_statement(&UNIQUE_VIOLATION).await;

    let refused = in_a_job_on(
        &url,
        Repo::<note::Entity>::insert_unscoped(note::ActiveModel {
            id: Set(uuid::Uuid::now_v7()),
            body: Set(MARKED.to_owned()),
        }),
    )
    .await
    .expect_err("the double refuses every statement");

    assert!(
        matches!(&refused, RepoError::UniqueViolation(key)
            if key.constraint() == Some("notes_body_key") && key.table() == Some("notes")),
        "{refused:?}"
    );
    let printed = nest_rs_testing::printed(&refused);
    assert!(
        printed.contains("23505") && printed.contains("notes_body_key"),
        "said by its SQLSTATE and its key: {printed}"
    );
    assert!(!printed.contains(MARKED), "{printed}");
}

/// A read whose value the server refuses is said by its SQLSTATE; no reader of it
/// prints the message quoting the value.
#[tokio::test]
async fn a_read_the_server_refuses_is_handed_out_naming_no_value_it_quoted() {
    let (url, _root) = crate::postgres_refusing_every_statement(&INVALID_TEXT).await;

    let refused = in_a_job_on(&url, Repo::<note::Entity>::unscoped().one())
        .await
        .expect_err("the double refuses every statement");

    assert!(matches!(refused, RepoError::Other { .. }), "{refused:?}");
    let printed = nest_rs_testing::printed(&refused);
    assert!(printed.contains("22P02"), "said by its SQLSTATE: {printed}");
    assert!(!printed.contains(MARKED), "{printed}");
}
