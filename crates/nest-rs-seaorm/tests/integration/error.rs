//! `src/error.rs` in process: what a GraphQL operation answers when its database
//! fails, against a pool with no server behind it — a refused connection is a
//! database failure like any other, and no Postgres is needed to provoke one.

use nest_rs_graphql::GraphqlView;
use nest_rs_http::EndpointGuards;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::problem::code;
use nest_rs_core::{Layer, OPAQUE_CLIENT_MESSAGE, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, crud, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_seaorm::{
    Creatable, CrudService, SeaOrmDatabaseModule, SeaOrmModule, ServiceError, expose,
};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::{ConnectOptions, Database, DbErr, RuntimeErr};
use serde_json::{Value, json};

mod widget {
    use super::*;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Widget", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable]
#[derive(Default)]
struct WidgetsService;

impl CrudService for WidgetsService {
    type Entity = widget::Entity;
}

impl Creatable for WidgetsService {
    type Create = widget::CreateWidget;
}

#[injectable]
#[derive(Default)]
struct EveryWidget;

impl AbilityFactory for EveryWidget {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, widget::Entity);
        ab.can(Action::Create, widget::Entity);
    }
}

#[injectable]
#[derive(Default)]
struct AlwaysAuthenticated;

impl Layer for AlwaysAuthenticated {}

#[async_trait]
impl Guard for AlwaysAuthenticated {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(crate::Anyone);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AlwaysAuthenticated {}

#[resolver]
struct WidgetsResolver {
    #[inject]
    svc: Arc<WidgetsService>,
}

#[crud(
    service = svc,
    entity = widget::Entity,
    output = widget::Widget,
    create = widget::CreateWidget,
    ops = [list, create],
)]
impl WidgetsResolver {}

/// A service method whose database failure carries a value, answered through
/// `ServiceError` rather than `#[crud]`.
#[resolver]
struct LedgerResolver;

#[operations]
impl LedgerResolver {
    #[query]
    #[public]
    async fn ledger(&self) -> Result<String, ServiceError> {
        Err(ServiceError::from(DbErr::Exec(RuntimeErr::Internal(
            "invalid input syntax for type uuid: \"secret-123\"".into(),
        ))))
    }

    #[query]
    #[public]
    async fn entry(&self) -> Result<String, ServiceError> {
        Err(ServiceError::from(DbErr::RecordNotUpdated))
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(None),
        SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        WidgetsService,
        EveryWidget,
        AbilityGuard<EveryWidget>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<EveryWidget>)>,
        WidgetsResolver,
        LedgerResolver,
    ],
)]
struct WidgetsModule;

/// The app over a pool whose every acquire is refused within a fifth of a
/// second: nothing listens on port 1.
async fn boot() -> TestApp {
    let mut options = ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
    options
        .connect_lazy(true)
        .acquire_timeout(Duration::from_millis(200));
    let db = Database::connect(options)
        .await
        .expect("a lazy pool opens nothing");
    TestApp::builder()
        .provide(db)
        .module::<WidgetsModule>()
        .build()
        .await
        .expect("the app boots: the server's absence is a runtime fact")
}

async fn first_error(app: &TestApp, query: &str) -> Value {
    let body: Value = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": query }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();
    body["errors"][0].clone()
}

#[tokio::test]
async fn a_crud_operation_the_database_fails_answers_opaquely_and_files_its_error_once() {
    let logs = LogCapture::install();
    let app = boot().await;

    let error = first_error(&app, "{ widgets { id label } }").await;
    assert_eq!(error["message"], OPAQUE_CLIENT_MESSAGE, "{error}");
    assert_eq!(
        error["extensions"]["code"],
        code::INTERNAL.as_str(),
        "{error}"
    );

    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "crud operation failed");
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the half the client never sees: {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_service_error_from_the_database_answers_its_class_and_never_its_value() {
    let logs = LogCapture::install();
    let app = boot().await;

    let error = first_error(&app, "{ ledger }").await;
    assert_eq!(error["message"], OPAQUE_CLIENT_MESSAGE, "{error}");
    assert_eq!(
        error["extensions"]["code"],
        code::INTERNAL.as_str(),
        "{error}"
    );
    let filed = logs.expect_one(nest_rs_graphql::TARGET, "graphql operation failed");
    let said = filed.field("error").unwrap_or_default();
    assert!(said.contains("Execution Error"), "{said}");
    assert!(!said.contains("secret-123"), "{said}");

    let error = first_error(&app, "{ entry }").await;
    assert_eq!(error["message"], "not found", "{error}");
    assert_eq!(
        error["extensions"]["code"],
        code::NOT_FOUND.as_str(),
        "{error}"
    );
}
