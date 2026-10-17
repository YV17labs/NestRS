//! `src/error.rs`'s GraphQL twin through a `#[crud]` create the database
//! refuses, over `POST /graphql`: a unique violation is the client's `CONFLICT`
//! and no incident, any other refusal the opaque `INTERNAL` filed once — and
//! neither answer nor line carries the server's sentence or the input it quotes.

use nest_rs_graphql::GraphqlView;
use nest_rs_http::EndpointGuards;
use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::problem::code;
use nest_rs_core::{Layer, OPAQUE_CLIENT_MESSAGE, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, crud, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_seaorm::{
    Creatable, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, expose,
};
use nest_rs_testing::{LogCapture, TestApp};
use poem::http::StatusCode;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};

const TABLE: &str = "crud_probe_gizmos";
const UNIQUE: &str = "crud_probe_gizmos_email_key";

mod gizmo {
    use super::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Gizmo", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "crud_probe_gizmos")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub email: String,
        /// Optional here and `NOT NULL` in the table, so a create leaving it out
        /// is a refusal the server words around the row it refused.
        #[expose(input(create))]
        pub note: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable]
#[derive(Default)]
struct GizmosService;

impl CrudService for GizmosService {
    type Entity = gizmo::Entity;
}

impl Creatable for GizmosService {
    type Create = gizmo::CreateGizmo;
}

#[injectable]
#[derive(Default)]
struct EveryGizmo;

impl AbilityFactory for EveryGizmo {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, gizmo::Entity);
        ab.can(Action::Create, gizmo::Entity);
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

type Bridge = EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<EveryGizmo>)>;

#[resolver]
struct GizmosResolver {
    #[inject]
    svc: Arc<GizmosService>,
}

#[crud(
    service = svc,
    entity = gizmo::Entity,
    output = gizmo::Gizmo,
    create = gizmo::CreateGizmo,
    ops = [get, create],
)]
impl GizmosResolver {}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        GizmosService,
        EveryGizmo,
        AbilityGuard<EveryGizmo>,
        AlwaysAuthenticated,
        Bridge,
        GizmosResolver,
    ],
)]
struct GizmosModule;

const CREATE: &str =
    "mutation($input: CreateGizmo!) { createGizmo(input: $input) { id email note } }";

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        TABLE,
        &format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (
                id UUID PRIMARY KEY,
                email TEXT CONSTRAINT {UNIQUE} UNIQUE NOT NULL,
                note TEXT NOT NULL
            );"
        ),
    )
    .await;
    conn
}

async fn boot() -> TestApp {
    TestApp::for_module::<GizmosModule>()
        .await
        .expect("the subgraph boots")
}

/// The mutation's status and its body, read whole so an assertion can search it.
async fn create(app: &TestApp, input: Value) -> (StatusCode, String) {
    let response = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": CREATE, "variables": { "input": input } }))
        .send()
        .await
        .0;
    let status = response.status();
    let body = response
        .into_body()
        .into_string()
        .await
        .expect("a response body");
    (status, body)
}

async fn rows_with(conn: &DatabaseConnection, email: &str) -> i64 {
    conn.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("SELECT COUNT(*) FROM {TABLE} WHERE email = $1"),
        [email.into()],
    ))
    .await
    .expect("the probe rows are counted")
    .expect("one row")
    .try_get_by_index(0)
    .expect("an i64 count")
}

fn errors_filed(logs: &LogCapture) -> Vec<String> {
    logs.events()
        .into_iter()
        .filter(|event| event.level == "error")
        .map(|event| format!("{} {}", event.target, event.message))
        .collect()
}

#[tokio::test]
async fn a_create_hitting_a_unique_violation_answers_conflict_and_files_no_error() {
    let conn = db().await;
    let logs = LogCapture::install();
    let app = boot().await;
    let email = format!("secret-{}@example.com", Uuid::now_v7());

    let (status, body) = create(&app, json!({ "email": email, "note": "first" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains("\"errors\""), "the first one lands: {body}");

    let (status, body) = create(&app, json!({ "email": email, "note": "second" })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the refused INSERT rolled back its own savepoint, not the request: {body}",
    );
    let answer: Value = serde_json::from_str(&body).expect("a GraphQL body");
    assert_eq!(answer["errors"][0]["message"], "conflict", "{body}");
    assert_eq!(
        answer["errors"][0]["extensions"]["code"],
        code::CONFLICT.as_str(),
        "{body}",
    );
    for withheld in [UNIQUE, TABLE, email.as_str(), "23505"] {
        assert!(!body.contains(withheld), "{withheld} in {body}");
    }
    assert_eq!(
        errors_filed(&logs),
        Vec::<String>::new(),
        "a client's conflict is no incident",
    );

    let committed = rows_with(&conn, &email).await;
    assert_eq!(committed, 1, "the first create alone committed");
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("DELETE FROM {TABLE} WHERE email = $1"),
        [email.into()],
    ))
    .await
    .expect("the probe row is removed");
}

#[tokio::test]
async fn a_create_the_database_refuses_otherwise_answers_opaquely_and_files_it_once() {
    let conn = db().await;
    let logs = LogCapture::install();
    let app = boot().await;
    let email = format!("secret-{}@example.com", Uuid::now_v7());

    let (status, body) = create(&app, json!({ "email": email })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answer: Value = serde_json::from_str(&body).expect("a GraphQL body");
    assert_eq!(
        answer["errors"][0]["message"], OPAQUE_CLIENT_MESSAGE,
        "{body}"
    );
    assert_eq!(
        answer["errors"][0]["extensions"]["code"],
        code::INTERNAL.as_str(),
        "{body}",
    );
    for withheld in [TABLE, "note", email.as_str(), "23502"] {
        assert!(!body.contains(withheld), "{withheld} in {body}");
    }

    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "crud operation failed");
    assert_eq!(event.level, "error");
    let filed = format!("{} {}", event.target, event.message);
    assert_eq!(
        errors_filed(&logs),
        [filed],
        "filed once, by the crud mapping alone"
    );
    let said = event.field("error").unwrap_or_default();
    for named in [
        "database error 23502".to_owned(),
        format!("table \"{TABLE}\""),
        "column \"note\"".to_owned(),
    ] {
        assert!(said.contains(&named), "{named} in {said}");
    }
    assert!(
        !said.contains(&email),
        "the server's detail quotes the refused row: {said}"
    );

    assert_eq!(rows_with(&conn, &email).await, 0, "nothing was written");
}
