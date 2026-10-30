//! Covers `src/graphql/bind.rs`: the service `bind = Service` loads through is
//! inside the access contract the boot checks, as a route's is, and an
//! operation no guard attaches an ability to answers opaquely, the remedy on
//! the operator's line.

use nest_rs_authz::{Read, Update};
use nest_rs_core::{AccessGraphError, injectable, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_seaorm::{Authorized, CrudService};
use nest_rs_testing::{LogCapture, TestApp};

mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl nest_rs_authz::WireModelDefaults for note::Entity {}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

#[module(providers = [NotesService])]
struct NotesModule;

#[resolver]
struct NotesResolver;

#[operations]
impl NotesResolver {
    #[mutation]
    #[authorize(Update, bind = NotesService)]
    async fn touch_note(&self, note: Authorized<Update, note::Entity>) -> bool {
        let _ = note;
        true
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [NotesResolver])]
struct NotesGraphqlModule;

/// The service's module is reached through a sibling, never imported by the
/// resolver's own.
#[module(imports = [NotesModule, NotesGraphqlModule])]
struct StrayNotesRoot;

#[tokio::test]
async fn a_bound_service_whose_module_is_not_imported_fails_the_boot() {
    let refused = TestApp::for_module::<StrayNotesRoot>()
        .await
        .err()
        .expect("a `bind` over an unimported service must fail the boot");
    let access = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
    assert_eq!(access.consumer, "NotesResolver");
    assert_eq!(access.owner, "NotesModule");
}

#[injectable]
#[derive(Default)]
struct ListedService;

impl CrudService for ListedService {
    type Entity = note::Entity;
}

/// Bound, and behind no guard: nothing attaches the ability its gate reads.
#[resolver]
struct UngatedResolver;

#[operations]
impl UngatedResolver {
    #[query]
    #[authorize(Read, bind = ListedService)]
    async fn listed_note(&self, note: Authorized<Read, note::Entity>) -> bool {
        let _ = note;
        true
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [ListedService, UngatedResolver],
)]
struct UnboundModule;

#[tokio::test]
async fn a_bound_field_with_no_unit_of_work_binding_is_refused_at_boot_naming_it() {
    let refused = TestApp::for_module::<UnboundModule>()
        .await
        .err()
        .expect("the subject loads through `Repo`, which no unit of work would serve");
    let missing = refused
        .downcast::<nest_rs_core::MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency, missing.remedy),
        (
            "UngatedResolver",
            "dyn UnitOfWork",
            Some(nest_rs_database::BACKEND_REMEDY)
        ),
    );
}

#[module(
    imports = [GraphqlModule::for_root(None), nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [ListedService, UngatedResolver],
)]
struct UngatedModule;

async fn answer(app: &TestApp, field: &str) -> serde_json::Value {
    let query = format!(r#"{{ {field}(id: "{}") }}"#, uuid::Uuid::now_v7());
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.json().await.value().deserialize()
}

#[tokio::test]
async fn a_bind_with_no_ability_answers_opaquely_and_files_the_remedy() {
    let app = crate::over_an_unconnected_pool()
        .module::<UngatedModule>()
        .build()
        .await
        .expect("boots");
    let logs = LogCapture::install();

    let body = answer(&app, "listedNote").await;

    assert_eq!(
        body["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "INTERNAL",
        "{body}"
    );
    assert!(!body.to_string().contains("Ability"), "{body}");
    let line = logs.expect_one(nest_rs_authz::TARGET, "authorization denied");
    assert_eq!(line.level, "error");
    assert!(line.field("remedy").is_some(), "{:?}", line.fields);
}
