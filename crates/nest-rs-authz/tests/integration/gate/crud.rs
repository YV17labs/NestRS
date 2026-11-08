//! The class gate in front of every `#[crud]` operation: under an org-wide
//! `Manage`, a denial no `when` narrows refuses its action before the service
//! runs a statement. The database is one no statement can reach, so a request
//! the gate let through would answer its `500`.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action};
use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HttpView, Method, StatusCode, controller, crud};
use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable};
use nest_rs_testing::TestApp;

mod note {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub org_id: i32,
        #[expose(input(create, update))]
        pub title: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// The header naming the action a denial no `when` narrows bars.
const DENY: &str = "x-deny";

/// Manages the notes of org 7, but for the action [`DENY`] names, which a
/// `cannot` claws back on every row.
#[injectable]
#[derive(Default, Layer)]
struct OrgManager;

#[async_trait]
impl Guard for OrgManager {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let denied = match HttpView::of(cx)?.request().header(DENY) {
            Some("read") => Action::Read,
            Some("create") => Action::Create,
            Some("update") => Action::Update,
            Some("delete") => Action::Delete,
            _ => Action::Manage,
        };
        let mut ab = AbilityBuilder::new();
        ab.can(Action::Manage, note::Entity)
            .when(|p| p.eq(note::Column::OrgId, 7));
        ab.cannot(denied, note::Entity);
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ab.build().expect("valid test ability"),
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for OrgManager {}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

impl Creatable for NotesService {
    type Create = note::CreateNote;
}

impl Updatable for NotesService {
    type Update = note::UpdateNote;
}

impl Deletable for NotesService {}

#[controller(path = "/notes")]
#[use_guards(OrgManager)]
struct NotesController {
    #[inject]
    svc: Arc<NotesService>,
}

#[crud(service = NotesService)]
impl NotesController {}

#[module(
    imports = [nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [OrgManager, NotesService, NotesController],
)]
struct NotesModule;

const NOTE: &str = "/notes/0195b4a6-4c1c-7d3e-8f00-000000000001";

#[tokio::test]
async fn every_crud_operation_a_denial_bars_is_refused_before_a_statement() {
    let app = TestApp::builder()
        .provide(sea_orm::DatabaseConnection::default())
        .provide(nest_rs_seaorm::SeaOrmConfig::default())
        .module::<NotesModule>()
        .build()
        .await
        .expect("the notes module boots");

    let operations = [
        (Method::POST, "/notes", "create"),
        (Method::GET, "/notes", "read"),
        (Method::GET, NOTE, "read"),
        (Method::PUT, NOTE, "update"),
        (Method::DELETE, NOTE, "delete"),
    ];
    for (method, path, action) in operations {
        for denied in [action, "manage"] {
            let refused = app
                .http()
                .request(method.clone(), path)
                .header(DENY, denied)
                .body_json(&serde_json::json!({ "title": "minutes" }))
                .send()
                .await;
            let status = refused.0.status();
            let body = refused.0.into_body().into_string().await.expect("a body");
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} under `cannot({denied})`: {body}",
            );
            let problem: serde_json::Value =
                serde_json::from_str(&body).expect("a problem document");
            assert_eq!(problem["code"], "FORBIDDEN", "{method} {path}: {problem}");
        }
    }
}
