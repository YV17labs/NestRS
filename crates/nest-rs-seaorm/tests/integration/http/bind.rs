//! Covers `src/http/bind.rs`: the subject a route's `#[authorize(A, bind = S)]`
//! loads. The service is inside the access contract the boot checks; the
//! posture's class gate runs before the load, and the load refuses an id that
//! is not a UUID v7 before it reads anything.

use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{
    AccessGraphError, App, Discovery, Layer, UnitContext, async_trait, injectable, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HttpControllerMeta, HttpView, StatusCode, controller, routes};
use nest_rs_seaorm::{Authorized, CrudService, SeaOrmDatabaseModule};
use nest_rs_testing::TestApp;

mod note {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable(scope = request)]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

#[module(providers = [NotesService])]
struct NotesModule;

/// Grants `Read` on notes to a caller sending `x-read`, nothing otherwise.
#[injectable]
#[derive(Default)]
struct Grants;

impl Layer for Grants {}

#[async_trait]
impl Guard for Grants {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let read = HttpView::of(cx)?.request().header("x-read").is_some();
        let mut ability = AbilityBuilder::new();
        if read {
            ability.can(Action::Read, note::Entity);
        }
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ability
                .build()
                .map_err(|_malformed| Denial::internal("malformed rules"))?,
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for Grants {}

#[controller(path = "/notes")]
#[use_guards(Grants)]
struct NotesController;

#[routes]
impl NotesController {
    #[get("/{id}")]
    #[authorize(Read, bind = NotesService)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> String {
        note.id.to_string()
    }
}

#[module(providers = [Grants, NotesController])]
struct NotesHttpModule;

/// The service's module is reached through a sibling, never imported by the
/// controller's own.
#[module(imports = [NotesModule, NotesHttpModule])]
struct StrayNotesRoot;

#[tokio::test]
async fn a_bound_service_whose_module_is_not_imported_fails_the_boot() {
    let access = match App::builder().module::<StrayNotesRoot>().build().await {
        Ok(_) => panic!("a `bind` over an unimported service must fail the boot"),
        Err(err) => err
            .downcast::<AccessGraphError>()
            .unwrap_or_else(|err| panic!("not the access violation: {err:#}")),
    };
    assert_eq!(access.consumer, "NotesController");
    assert_eq!(access.owner, "NotesModule");
}

#[module(imports = [NotesModule], providers = [Grants, NotesController])]
struct UnboundNotesModule;

#[tokio::test]
async fn a_bound_route_with_no_unit_of_work_binding_is_refused_at_boot_naming_it() {
    let Err(refused) = App::builder().module::<UnboundNotesModule>().build().await else {
        panic!("the subject loads through `Repo`, which no unit of work would serve");
    };
    let missing = refused
        .downcast::<nest_rs_core::MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency, missing.remedy),
        (
            "NotesController",
            "dyn UnitOfWork",
            Some(nest_rs_database::BACKEND_REMEDY)
        ),
    );
}

#[module(
    imports = [NotesModule, SeaOrmDatabaseModule],
    providers = [Grants, NotesController],
)]
struct BoundNotesModule;

async fn app() -> TestApp {
    crate::over_an_unconnected_pool()
        .module::<BoundNotesModule>()
        .build()
        .await
        .expect("a bound route over an imported service boots")
}

#[tokio::test]
async fn the_class_gate_refuses_before_the_subject_loads() {
    let app = app().await;

    app.http()
        .get(format!("/notes/{}", uuid::Uuid::new_v4()))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_id_that_is_not_v7_is_refused_before_anything_is_read() {
    let app = app().await;

    let refused = app
        .http()
        .get(format!("/notes/{}", uuid::Uuid::new_v4()))
        .header("x-read", "")
        .send()
        .await;

    refused.assert_status(StatusCode::BAD_REQUEST);
    let body = refused.0.into_body().into_string().await.expect("a body");
    assert!(body.contains(nest_rs_core::UUID_V7_REQUIRED), "{body}");
}

#[tokio::test]
async fn a_bound_route_is_documented_as_masked() {
    let app = app().await;
    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();

    let route = &controllers[0].meta.routes()[0];

    assert!(route.masked(), "the posture arms the response mask");
    assert!(
        route.path_params().is_empty(),
        "the subject's id is no `Path<T>` the document types"
    );
}

/// The subject's id is the one parameter of the controller's path; the
/// route's own names none.
#[controller(path = "/notes/{id}")]
#[use_guards(Grants)]
struct NoteSummaryController;

#[routes]
impl NoteSummaryController {
    #[get("/summary")]
    #[authorize(Read, bind = NotesService)]
    async fn summary(&self, note: Authorized<Read, note::Entity>) -> String {
        note.id.to_string()
    }
}

#[module(
    imports = [NotesModule, SeaOrmDatabaseModule],
    providers = [Grants, NoteSummaryController],
)]
struct NoteSummaryModule;

#[tokio::test]
async fn a_bound_route_reads_the_id_its_controller_path_names() {
    let app = crate::over_an_unconnected_pool()
        .module::<NoteSummaryModule>()
        .build()
        .await
        .expect("a route bound on its controller's one parameter boots");

    let refused = app
        .http()
        .get(format!("/notes/{}/summary", uuid::Uuid::new_v4()))
        .header("x-read", "")
        .send()
        .await;

    refused.assert_status(StatusCode::BAD_REQUEST);
    let body = refused.0.into_body().into_string().await.expect("a body");
    assert!(body.contains(nest_rs_core::UUID_V7_REQUIRED), "{body}");
}
