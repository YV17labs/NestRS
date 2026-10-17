//! Covers `src/http/bind.rs` at the boot: the service a `Bind<A, S>` parameter
//! loads through is inside the access contract, so a route that reaches it
//! through no import fails the boot rather than its first request.

use nest_rs_authz::Read;
use nest_rs_core::{AccessGraphError, App, injectable, module};
use nest_rs_http::{controller, routes};
use nest_rs_seaorm::{Bind, CrudService};

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

#[injectable(scope = request)]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

#[module(providers = [NotesService])]
struct NotesModule;

#[controller(path = "/notes")]
struct NotesController;

#[routes]
impl NotesController {
    #[get("/{id}")]
    async fn note(&self, note: Bind<Read, NotesService>) -> String {
        note.id.to_string()
    }
}

#[module(providers = [NotesController])]
struct NotesHttpModule;

/// The service's module is reached through a sibling, never imported by the
/// controller's own.
#[module(imports = [NotesModule, NotesHttpModule])]
struct StrayNotesRoot;

#[tokio::test]
async fn a_bound_service_whose_module_is_not_imported_fails_the_boot() {
    let access = match App::builder().module::<StrayNotesRoot>().build().await {
        Ok(_) => panic!("a `Bind` over an unimported service must fail the boot"),
        Err(err) => err
            .downcast::<AccessGraphError>()
            .unwrap_or_else(|err| panic!("not the access violation: {err:#}")),
    };
    assert_eq!(access.consumer, "NotesController");
    assert_eq!(access.owner, "NotesModule");
}
