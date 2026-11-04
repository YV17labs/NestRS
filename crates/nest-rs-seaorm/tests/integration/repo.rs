//! `src/repo.rs`'s saves and `src/service.rs`'s create under a route gated on a
//! `#[subject]`: no mask shapes its reply, so `UnmaskedReply` withholds the
//! ability its guard attached, and no row could pass their scope. They answer
//! that before the entity's `before_save` runs or a statement is sent, on a pool
//! that dials nothing, where a statement sent would answer its connection error.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Manage, subject};
use nest_rs_core::{injectable, module};
use nest_rs_http::{Json, StatusCode, controller, routes};
use nest_rs_seaorm::{Creatable, CreateModel, CrudService, Repo, SeaOrmDatabaseModule};
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
}

fn reply(written: Result<note::Model, sea_orm::DbErr>) -> Value {
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
        json!({ "hooks": 0, "written": { "Err": "RecordNotUpdated" } }),
    );
}

#[tokio::test]
async fn a_create_on_a_route_gated_on_a_subject_runs_no_hook_and_sends_nothing() {
    assert_eq!(
        answered("/reports/created").await,
        json!({ "hooks": 0, "written": { "Err": "RecordNotInserted" } }),
    );
}
