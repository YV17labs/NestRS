//! `src/error.rs`'s GraphQL twin through a `#[crud]` create the database
//! refuses, over `POST /graphql`: a unique violation is the client's `CONFLICT`
//! naming the input's field, and no incident, any other refusal the opaque
//! `INTERNAL` filed once — and neither answer nor line carries the server's
//! sentence or the input it quotes.
//! A caller a denial no `when` narrows bars meets none of it: the class gate
//! refuses before Postgres sees a statement.

use nest_rs_graphql::GraphqlView;
use nest_rs_http::EndpointGuards;
use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::problem::code;
use nest_rs_core::{OPAQUE_CLIENT_MESSAGE, injectable, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, crud, resolver};
use nest_rs_http::StatusCode;
use nest_rs_seaorm::{
    Creatable, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, expose,
};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};

use crate::always_authenticated::AlwaysAuthenticated;

const TABLE: &str = "crud_probe_gizmos";
const UNIQUE: &str = "crud_probe_gizmos_email_key";

mod gizmo {
    use super::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Gizmo", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

/// Manages every gizmo but may create and read none: CASL's grant clawed back.
#[injectable]
#[derive(Default)]
struct BarredGizmo;

impl AbilityFactory for BarredGizmo {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Manage, gizmo::Entity);
        ab.cannot(Action::Create, gizmo::Entity);
        ab.cannot(Action::Read, gizmo::Entity);
    }
}

#[resolver]
struct GizmosResolver {
    #[inject]
    svc: Arc<GizmosService>,
}

#[crud(service = GizmosService, ops = [get, create])]
impl GizmosResolver {}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
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
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<EveryGizmo>)>,
        GizmosResolver,
    ],
)]
struct GizmosModule;

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        GizmosService,
        BarredGizmo,
        AbilityGuard<BarredGizmo>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<BarredGizmo>)>,
        GizmosResolver,
    ],
)]
struct BarredGizmosModule;

/// A key over a column of two words, so its field's Rust name and its schema's differ.
mod plate {
    use super::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Plate", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "crud_probe_plates")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub serial_code: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[injectable]
#[derive(Default)]
struct PlatesService;

impl CrudService for PlatesService {
    type Entity = plate::Entity;
}

impl Creatable for PlatesService {
    type Create = plate::CreatePlate;
}

#[injectable]
#[derive(Default)]
struct EveryPlate;

impl AbilityFactory for EveryPlate {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, plate::Entity);
        ab.can(Action::Create, plate::Entity);
    }
}

#[resolver]
struct PlatesResolver {
    #[inject]
    svc: Arc<PlatesService>,
}

#[crud(service = PlatesService, ops = [get, create])]
impl PlatesResolver {}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        PlatesService,
        EveryPlate,
        AbilityGuard<EveryPlate>,
        AlwaysAuthenticated,
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, AbilityGuard<EveryPlate>)>,
        PlatesResolver,
    ],
)]
struct PlatesModule;

const CREATE: &str =
    "mutation($input: CreateGizmo!) { createGizmo(input: $input) { id email note } }";

const GET: &str = "query($id: String!) { gizmo(id: $id) { id } }";

const CREATE_PLATE: &str = "mutation($input: CreatePlate!) { createPlate(input: $input) { id } }";

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
    operation(app, CREATE, json!({ "input": input })).await
}

/// An operation's status and its body, read whole.
async fn operation(app: &TestApp, query: &str, variables: Value) -> (StatusCode, String) {
    let response = app
        .http()
        .post("/graphql")
        .body_json(&json!({ "query": query, "variables": variables }))
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
    logs.events_at(tracing::Level::ERROR)
        .into_iter()
        .map(|event| format!("{} {}", event.target, event.message))
        .collect()
}

#[tokio::test]
async fn a_conflict_on_a_key_of_two_words_names_its_field_as_the_schema_spells_it() {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "crud_probe_plates",
        "CREATE TABLE IF NOT EXISTS crud_probe_plates (
            id UUID PRIMARY KEY,
            serial_code TEXT CONSTRAINT crud_probe_plates_serial_code_key UNIQUE NOT NULL
        );",
    )
    .await;
    let app = TestApp::for_module::<PlatesModule>()
        .await
        .expect("the subgraph boots");
    let input = json!({ "input": { "serialCode": format!("serial-{}", Uuid::now_v7()) } });

    let (_, first) = operation(&app, CREATE_PLATE, input.clone()).await;
    assert!(
        !first.contains("\"errors\""),
        "the first one lands: {first}"
    );

    let (_, second) = operation(&app, CREATE_PLATE, input).await;
    let answer: Value = serde_json::from_str(&second).expect("a GraphQL body");
    assert_eq!(
        answer["errors"][0]["extensions"]["errors"],
        json!({ "serialCode": [{ "code": "unique" }] }),
        "the key a client matches to the field it sent: {second}",
    );
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
    assert_eq!(
        answer["errors"][0]["extensions"]["errors"],
        json!({ "email": [{ "code": "unique" }] }),
        "the conflict names the input's field whose value is taken: {body}",
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

/// A taken address and a missing note are what Postgres would refuse, and a
/// row's presence what a load would tell: under a denial no `when` narrows,
/// each answers the gate's `FORBIDDEN` before any of them is asked.
#[tokio::test]
async fn a_caller_a_denial_bars_outright_reaches_no_statement() {
    let conn = db().await;
    let logs = LogCapture::install();
    let granted = boot().await;
    let barred = TestApp::for_module::<BarredGizmosModule>()
        .await
        .expect("the subgraph boots");
    let email = format!("taken-{}@example.com", Uuid::now_v7());
    let (_, body) = create(&granted, json!({ "email": email, "note": "first" })).await;
    let landed: Value = serde_json::from_str(&body).expect("a GraphQL body");
    let id = landed["data"]["createGizmo"]["id"]
        .as_str()
        .expect("the first create lands")
        .to_owned();

    let fresh = format!("fresh-{}@example.com", Uuid::now_v7());
    for (input, would_be) in [
        (json!({ "email": email, "note": "second" }), "CONFLICT"),
        (json!({ "email": fresh }), "INTERNAL"),
    ] {
        let (status, body) = create(&barred, input).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let answer: Value = serde_json::from_str(&body).expect("a GraphQL body");
        assert_eq!(
            answer["errors"][0]["extensions"]["code"],
            code::FORBIDDEN.as_str(),
            "refused at the gate, not the database's {would_be}: {body}",
        );
    }
    for id in [id.clone(), Uuid::now_v7().to_string()] {
        let (_, body) = operation(&barred, GET, json!({ "id": id })).await;
        let answer: Value = serde_json::from_str(&body).expect("a GraphQL body");
        assert_eq!(
            answer["errors"][0]["extensions"]["code"],
            code::FORBIDDEN.as_str(),
            "the same answer whether the row exists or not: {body}",
        );
    }
    assert_eq!(
        errors_filed(&logs),
        Vec::<String>::new(),
        "a denial is no incident"
    );
    assert_eq!(rows_with(&conn, &fresh).await, 0);
    assert_eq!(
        rows_with(&conn, &email).await,
        1,
        "the granted create alone"
    );
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("DELETE FROM {TABLE} WHERE email = $1"),
        [email.into()],
    ))
    .await
    .expect("the probe row is removed");
}
