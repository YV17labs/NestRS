//! Covers `src/graphql/bind.rs`: the service `bind = Service` loads through is
//! inside the access contract the boot checks, as a route's is, and an
//! operation its guards attached no ability to answers opaquely, the remedy on
//! the operator's line. A visitor's `#[public(A, bind = S)]` loads and judges by
//! its grant, as `#[authorize]`'s does a principal's.

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Read, Update};
use nest_rs_core::{AccessGraphError, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::EndpointGuards;
use nest_rs_seaorm::{Access, Authorized, CrudService, RepoError};
use nest_rs_testing::{LogCapture, TestApp};

use crate::always_authenticated::AlwaysAuthenticated;

mod note {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: Option<String>,
        #[expose]
        pub body: Option<String>,
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

#[module(imports = [GraphqlModule], providers = [NotesResolver])]
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

/// Bound, and nothing attaches the ability its gate reads.
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
    imports = [GraphqlModule],
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
        (
            missing.consumer,
            missing.dependency,
            missing.remedy.as_deref()
        ),
        (
            "UngatedResolver",
            "dyn UnitOfWork",
            Some(nest_rs_database::BACKEND_REMEDY)
        ),
    );
}

/// Answers for an ability and attaches none, what the boot cannot tell apart.
#[injectable]
#[derive(Default, Layer)]
struct AttachesNothing;

#[async_trait]
impl Guard for AttachesNothing {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AttachesNothing {}

#[module(
    imports = [GraphqlModule, nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [
        ListedService,
        UngatedResolver,
        AttachesNothing,
        EndpointGuards<GraphqlView, (AttachesNothing,)>,
    ],
)]
struct UngatedModule;

#[module(
    imports = [GraphqlModule, nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [ListedService, UngatedResolver],
)]
struct UnreachedModule;

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

/// Nothing to attach what its gate reads, so every call would answer opaquely.
#[tokio::test]
async fn a_bind_no_ability_guard_reaches_refuses_the_boot() {
    let Err(refused) = crate::over_an_unconnected_pool()
        .module::<UnreachedModule>()
        .build()
        .await
    else {
        panic!("no guard attaches the ability its gate reads");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains("query listed_note") && said.contains("AbilityGuard"),
        "{said}"
    );
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

#[injectable]
#[derive(Default)]
struct CancelledService;

#[async_trait]
impl CrudService for CancelledService {
    type Entity = note::Entity;

    async fn access(
        &self,
        _action: Action,
        _id: uuid::Uuid,
    ) -> Result<Access<note::Model>, RepoError> {
        Err(crate::error::refused("57014", "canceling statement due to statement timeout").into())
    }
}

#[injectable]
#[derive(Default)]
struct EveryNote;

impl AbilityFactory for EveryNote {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, note::Entity);
    }
}

#[resolver]
struct CancelledResolver;

#[operations]
impl CancelledResolver {
    #[query]
    #[authorize(Read, bind = CancelledService)]
    async fn cancelled_note(&self, note: Authorized<Read, note::Entity>) -> bool {
        let _ = note;
        true
    }
}

#[module(
    imports = [GraphqlModule, nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [
        CancelledService,
        EveryNote,
        AbilityGuard<EveryNote>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<EveryNote>)>,
        CancelledResolver,
    ],
)]
struct CancelledModule;

/// A statement cancelled at its bound is the server unable to serve in time, the
/// class a route's bound load answers.
#[tokio::test]
async fn a_statement_cancelled_at_its_bound_answers_unavailable_filed_at_warn() {
    let app = crate::over_an_unconnected_pool()
        .module::<CancelledModule>()
        .build()
        .await
        .expect("boots");
    let logs = LogCapture::install();

    let body = answer(&app, "cancelledNote").await;

    assert_eq!(
        body["errors"][0]["extensions"]["code"], "UNAVAILABLE",
        "{body}"
    );
    let filed = logs.expect_one(nest_rs_graphql::TARGET, "graphql operation unavailable");
    assert_eq!(filed.level, "warn");
    assert!(
        filed
            .field("error")
            .is_some_and(|error| error.contains("57014")),
        "{:?}",
        filed.fields,
    );
}

/// A load finding a row whose title a visitor may read and whose body it may not.
#[injectable]
#[derive(Default)]
struct VisitedService;

#[async_trait]
impl CrudService for VisitedService {
    type Entity = note::Entity;

    async fn access(
        &self,
        action: Action,
        id: uuid::Uuid,
    ) -> Result<Access<note::Model>, RepoError> {
        assert_eq!(
            action,
            Action::Read,
            "the attribute's action reaches the load"
        );
        assert!(
            nest_rs_authz::current_ability().is_some(),
            "the load reads under the visitor's ability"
        );
        Ok(Access::Found(note::Model {
            id,
            title: Some("seen".to_owned()),
            body: Some("withheld".to_owned()),
        }))
    }
}

#[injectable]
#[derive(Default)]
struct VisitorGrants;

impl AbilityFactory for VisitorGrants {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, _ab: &mut AbilityBuilder) {}

    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, note::Entity)
            .fields([note::Column::Id, note::Column::Title]);
    }
}

#[resolver]
struct VisitedResolver;

#[operations]
impl VisitedResolver {
    #[query]
    #[public(Read, bind = VisitedService)]
    async fn visited_note(&self, note: Authorized<Read, note::Entity>) -> GqlResult<note::Note> {
        Ok(note::Note::from(&note.into_inner()))
    }
}

#[module(
    imports = [GraphqlModule, nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [
        VisitedService,
        VisitorGrants,
        AbilityGuard<VisitorGrants>,
        EndpointGuards<GraphqlView, (AbilityGuard<VisitorGrants>,)>,
        VisitedResolver,
    ],
)]
struct VisitedModule;

#[tokio::test]
async fn a_query_bound_for_a_visitor_loads_and_answers_by_its_grant() {
    let app = crate::over_an_unconnected_pool()
        .module::<VisitedModule>()
        .build()
        .await
        .expect("boots");
    let query = format!(
        r#"{{ visitedNote(id: "{}") {{ title body }} }}"#,
        uuid::Uuid::now_v7()
    );

    let body: serde_json::Value = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();

    assert_eq!(body["data"]["visitedNote"]["title"], "seen", "{body}");
    assert_eq!(
        body["data"]["visitedNote"]["body"],
        serde_json::Value::Null,
        "the row holds nothing the visitor's grant does not read: {body}"
    );
}
