//! `src/error.rs` in process: what a GraphQL operation answers when its database
//! fails, against a pool with no server behind it — a refused connection is a
//! database failure like any other, and no Postgres is needed to provoke one —
//! and where a route's opaque `ServiceError` is filed.

use nest_rs_graphql::GraphqlView;
use nest_rs_http::EndpointGuards;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::problem::code;
use nest_rs_core::{Layer, OPAQUE_CLIENT_MESSAGE, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, crud, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_http::{HttpModule, StatusCode, controller, routes};
use nest_rs_seaorm::{
    Creatable, CrudService, SeaOrmDatabaseModule, SeaOrmModule, ServiceError, expose,
};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::sqlx::error::{DatabaseError, ErrorKind};
use sea_orm::{ConnectOptions, Database, DbErr, RuntimeErr, SqlxError};
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

/// Postgres's own refusal as sqlx hands it over: a SQLSTATE, and a message
/// that may quote what the request sent.
#[derive(Debug)]
struct Refusal {
    code: &'static str,
    message: &'static str,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for Refusal {}

impl DatabaseError for Refusal {
    fn message(&self) -> &str {
        self.message
    }
    fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(self.code.into())
    }
    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

fn refused(code: &'static str, message: &'static str) -> DbErr {
    DbErr::Exec(RuntimeErr::SqlxError(Arc::new(SqlxError::Database(
        Box::new(Refusal { code, message }),
    ))))
}

#[controller(path = "/ledger")]
struct LedgerController;

#[routes]
impl LedgerController {
    #[get("/db")]
    async fn db(&self) -> Result<String, ServiceError> {
        Err(ServiceError::from(refused(
            "22P02",
            "invalid input syntax for type uuid: \"secret-123\"",
        )))
    }

    #[get("/masking")]
    async fn masking(&self) -> Result<String, ServiceError> {
        let failure = serde_json::from_str::<u64>(r#""secret-123""#).expect_err("not a number");
        Err(nest_rs_seaorm::__private::masking(&failure))
    }

    #[get("/internal")]
    async fn internal(&self) -> Result<String, ServiceError> {
        Err(ServiceError::internal_because(
            "could not sign the export",
            std::io::Error::other("the key store refused the connection"),
        ))
    }

    #[get("/cancelled")]
    async fn cancelled(&self) -> Result<String, ServiceError> {
        Err(ServiceError::from(refused(
            "57014",
            "canceling statement due to statement timeout",
        )))
    }

    #[get("/missing")]
    async fn missing(&self) -> Result<String, ServiceError> {
        Err(ServiceError::not_found("no such entry"))
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [LedgerController])]
struct LedgerHttpModule;

/// Every `warn` and `error` a request filed: its level, its target, its
/// message and the chain it said.
fn said(logs: &LogCapture) -> Vec<(String, String, String, String)> {
    logs.events()
        .into_iter()
        .filter(|event| event.level == "warn" || event.level == "error")
        .map(|event| {
            let error = event.field("error").unwrap_or_default();
            (event.level, event.target, event.message, error)
        })
        .collect()
}

/// A `ServiceError` a route answers opaquely is filed once, by the HTTP edge,
/// at the level of the status it sent — a statement Postgres cancelled at
/// `warn`, any other at `error` — its chain said without the value the
/// database quoted; a business refusal files nothing.
#[tokio::test]
async fn a_service_error_a_route_answers_opaquely_is_filed_once_by_the_edge() {
    let app = TestApp::for_module::<LedgerHttpModule>()
        .await
        .expect("the ledger app boots");

    for (path, status, level, message, cause) in [
        (
            "/ledger/db",
            StatusCode::INTERNAL_SERVER_ERROR,
            "error",
            "request failed",
            "22P02",
        ),
        (
            "/ledger/masking",
            StatusCode::INTERNAL_SERVER_ERROR,
            "error",
            "request failed",
            "response masking failed: invalid type: a string, expected u64",
        ),
        (
            "/ledger/internal",
            StatusCode::INTERNAL_SERVER_ERROR,
            "error",
            "request failed",
            "internal error: could not sign the export: the key store refused the connection",
        ),
        (
            "/ledger/cancelled",
            StatusCode::SERVICE_UNAVAILABLE,
            "warn",
            "request unavailable",
            "57014",
        ),
    ] {
        let logs = LogCapture::install();
        app.http().get(path).send().await.assert_status(status);
        let filed = said(&logs);
        let [(filed_level, target, filed_message, error)] = filed.as_slice() else {
            panic!("{path}: one line, {filed:#?}");
        };
        assert_eq!(
            [filed_level, target, filed_message],
            [level, nest_rs_http::target::HTTP, message],
            "{path}"
        );
        assert!(error.contains(cause), "{path}: {error}");
        assert!(!error.contains("secret-123"), "{path}: {error}");
    }

    let logs = LogCapture::install();
    app.http()
        .get("/ledger/missing")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    assert_eq!(said(&logs), [], "a refusal withholds nothing");
}
