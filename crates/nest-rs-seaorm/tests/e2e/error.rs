//! `src/error.rs` against Postgres: a refusal the server words around the input
//! it refused is said by its SQLSTATE and the names the server reported, and a
//! unique violation is a conflict, which a `#[crud]` write answers naming the
//! input's fields the key holds, read off the catalog at boot — what no unit
//! test can build, `PgDatabaseError` having no public constructor.

use std::time::Duration;

use nest_rs_core::{code, error_message, module};
use nest_rs_http::{StatusCode, header};
use nest_rs_seaorm::{RepoError, SeaOrmDatabaseModule, ServiceError};
use nest_rs_testing::TestApp;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use crate::harness::{ProbeController, ProbeHandler, ProbeMigrator, ProbeTables, answered};

const TABLE: &str = "error_probe_accounts";
const UNIQUE: &str = "error_probe_accounts_email_key";

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        TABLE,
        &format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (
                id UUID PRIMARY KEY,
                email TEXT CONSTRAINT {UNIQUE} UNIQUE NOT NULL
            );"
        ),
    )
    .await;
    conn
}

async fn insert(conn: &DatabaseConnection, email: Option<&str>) -> Result<(), DbErr> {
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("INSERT INTO {TABLE} (id, email) VALUES ($1, $2)"),
        [Uuid::now_v7().into(), email.map(str::to_owned).into()],
    ))
    .await
    .map(drop)
}

/// The job's error as the queue stores its dead-letter reason: the handler's
/// `anyhow` chain, boxed link by link, under a wrapper displaying it whole.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct DeadLettered(#[source] Box<dyn std::error::Error + Send + Sync>);

#[tokio::test]
async fn an_input_the_server_refused_is_said_by_its_sqlstate_and_never_quoted() {
    let conn = db().await;
    let refused = conn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT $1::uuid AS id",
            ["secret-123".into()],
        ))
        .await
        .expect_err("`secret-123` is no uuid");
    assert!(
        refused.to_string().contains("secret-123"),
        "the server quotes what it refused: {refused}",
    );

    let said = error_message(&refused);
    assert!(said.contains("database error 22P02"), "{said}");
    assert!(!said.contains("secret-123"), "{said}");

    let reason = error_message(&DeadLettered(nest_rs_core::boxed_error(
        anyhow::Error::from(refused.clone()).context("the import job failed"),
    )));
    assert!(reason.contains("22P02"), "{reason}");
    assert!(!reason.contains("secret-123"), "{reason}");

    let repo = RepoError::from(refused.clone());
    assert!(matches!(repo, RepoError::Other { .. }), "{repo:?}");
    let service = ServiceError::from(refused);
    assert!(matches!(service, ServiceError::Db(_)), "{service:?}");
    assert!(!error_message(&service).contains("secret-123"));
    let printed = nest_rs_testing::printed(&service);
    assert!(!printed.contains("secret-123"), "{printed}");
}

#[tokio::test]
async fn a_unique_violation_is_a_conflict_named_by_its_constraint_and_table() {
    let conn = db().await;
    let email = format!("taken-{}@example.com", Uuid::now_v7());
    insert(&conn, Some(&email))
        .await
        .expect("the first one lands");
    let taken = insert(&conn, Some(&email))
        .await
        .expect_err("the address is taken");

    let said = error_message(&taken);
    assert_eq!(
        said,
        format!(
            "Execution Error: database error 23505 on constraint \"{UNIQUE}\", table \"{TABLE}\""
        ),
    );
    let repo = RepoError::from(taken.clone());
    assert!(
        matches!(&repo, RepoError::UniqueViolation(key)
            if key.constraint() == Some(UNIQUE) && key.table() == Some(TABLE)),
        "{repo:?}"
    );
    let printed = nest_rs_testing::printed(&repo);
    assert!(
        !printed.contains(&email),
        "the server's detail quotes the row: {printed}"
    );
    assert!(
        matches!(ServiceError::from(taken), ServiceError::Conflict(_)),
        "a 409, not a 500",
    );

    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("DELETE FROM {TABLE} WHERE email = $1"),
        [email.into()],
    ))
    .await
    .expect("the probe row is removed");
}

#[tokio::test]
async fn a_not_null_violation_names_its_column() {
    let conn = db().await;
    let missing = insert(&conn, None).await.expect_err("an email is required");

    assert!(
        matches!(RepoError::from(missing.clone()), RepoError::Other { .. }),
        "no variant names a not-null violation"
    );
    let said = error_message(&missing);
    assert!(
        said.starts_with("Execution Error: database error 23502 on "),
        "{said}"
    );
    for named in [format!("table \"{TABLE}\""), "column \"email\"".to_owned()] {
        assert!(said.contains(&named), "{named} in {said}");
    }
}

#[module(
    imports = [SeaOrmDatabaseModule],
    providers = [ProbeController],
)]
struct BoundedProbeModule;

/// A statement Postgres cancels at its pool's bound, answered through a route:
/// the wait the client reads is the bound of the pool the request's unit ran
/// on, read where the edge renders the error.
#[tokio::test]
async fn a_route_whose_statement_is_cancelled_answers_its_pools_bound_as_the_wait() {
    let app = TestApp::builder()
        .provide(crate::module::bounding_statements(Duration::from_secs(1)))
        .provide(ProbeHandler::new(|| async {
            nest_rs_seaorm::__private::current_executor()
                .expect("the handler runs with an ambient executor")
                .execute_unprepared("SELECT pg_sleep(3)")
                .await
                .map_err(ServiceError::from)?;
            Ok(answered(StatusCode::OK))
        }))
        .module::<BoundedProbeModule>()
        .build()
        .await
        .expect("the probe app boots against live Postgres");

    let resp = app.http().post("/probe").send().await;
    resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    resp.assert_header(header::RETRY_AFTER, "1");
    let body = resp.json().await.value().deserialize::<serde_json::Value>();
    assert_eq!(body["code"], code::UNAVAILABLE.as_str(), "{body}");
    assert!(body.get("detail").is_none(), "a 5xx says no detail: {body}");
}

const HANDLES: &str = "conflict_probe_handles";

mod handle {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Handle")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "conflict_probe_handles")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        pub org_id: i32,
        #[expose(input(create, update))]
        pub email: String,
        #[expose(input(create, update))]
        pub slug: String,
        #[expose(input(create))]
        #[sea_orm(column_name = "display")]
        pub nickname: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct HandlesService;

impl nest_rs_seaorm::CrudService for HandlesService {
    type Entity = handle::Entity;
}

impl nest_rs_seaorm::Creatable for HandlesService {
    type Create = handle::CreateHandle;
}

impl nest_rs_seaorm::Updatable for HandlesService {
    type Update = handle::UpdateHandle;
}

/// Manages the handles of org 7: its `Create` grant pins `org_id`, which no
/// input carries.
#[nest_rs_core::injectable]
#[derive(Default)]
struct OrgSeven;

impl nest_rs_authz::AbilityFactory for OrgSeven {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut nest_rs_authz::AbilityBuilder) {
        ab.can(nest_rs_authz::Action::Manage, handle::Entity)
            .when(|p| p.eq(handle::Column::OrgId, 7));
    }
}

#[nest_rs_http::controller(path = "/handles")]
#[use_guards(
    crate::always_authenticated::AlwaysAuthenticated,
    nest_rs_authz::AbilityGuard<OrgSeven>
)]
struct HandlesController {
    #[inject]
    svc: std::sync::Arc<HandlesService>,
}

#[nest_rs_http::crud(service = HandlesService, ops = [create, update])]
impl HandlesController {}

#[module(
    imports = [SeaOrmDatabaseModule],
    providers = [
        HandlesService,
        OrgSeven,
        nest_rs_authz::AbilityGuard<OrgSeven>,
        crate::always_authenticated::AlwaysAuthenticated,
        HandlesController,
    ],
)]
struct HandlesModule;

/// The keys a conflict can name, which the app's boot reads: a constraint
/// carrying an `INCLUDE` column, a partial index whose other column a grant
/// pins, and an index over an expression.
struct HandlesTables;

impl ProbeTables for HandlesTables {
    fn ddl() -> String {
        format!(
            "CREATE TABLE {HANDLES} (
                id UUID PRIMARY KEY,
                org_id INT NOT NULL,
                email TEXT NOT NULL,
                slug TEXT NOT NULL,
                display TEXT NOT NULL,
                CONSTRAINT {HANDLES}_email_key UNIQUE (email) INCLUDE (slug)
            );
            CREATE UNIQUE INDEX {HANDLES}_slug_live ON {HANDLES} (org_id, slug) WHERE slug <> '';
            CREATE UNIQUE INDEX {HANDLES}_display_folded ON {HANDLES} (lower(display));"
        )
    }
}

async fn handles_app() -> TestApp {
    TestApp::builder()
        .module::<HandlesModule>()
        .ephemeral_database::<ProbeMigrator<HandlesTables>>()
        .build()
        .await
        .expect("the app boots on the probe database")
}

/// What a write answered: its status and its problem document.
async fn written(
    app: &TestApp,
    method: nest_rs_http::Method,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .http()
        .request(method, path)
        .body_json(&body)
        .send()
        .await;
    let status = resp.0.status();
    (
        status,
        resp.json().await.value().deserialize::<serde_json::Value>(),
    )
}

/// A `#[crud]` write a unique key refuses names the fields of its input the
/// key holds, in the shape a validation failure's fields take, and never a
/// value, an index or a table: the key's column a grant pins is no field the
/// client sent, an `INCLUDE` column is no part of the key, and an index over
/// an expression names nothing.
#[tokio::test]
async fn a_crud_write_a_unique_key_refuses_names_the_inputs_fields_it_holds() {
    use nest_rs_http::Method;
    use serde_json::json;

    let app = handles_app().await;
    let unique = json!([{ "code": "unique" }]);

    let (status, first) = written(
        &app,
        Method::POST,
        "/handles",
        json!({ "email": "ada@probe.test", "slug": "ada", "nickname": "Ada" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, second) = written(
        &app,
        Method::POST,
        "/handles",
        json!({ "email": "bob@probe.test", "slug": "bob", "nickname": "Bob" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");

    for (input, named) in [
        (
            json!({ "email": "ada@probe.test", "slug": "fresh", "nickname": "Fresh" }),
            Some(json!({ "email": unique })),
        ),
        (
            json!({ "email": "new@probe.test", "slug": "ada", "nickname": "Newer" }),
            Some(json!({ "slug": unique })),
        ),
        (
            json!({ "email": "other@probe.test", "slug": "other", "nickname": "ADA" }),
            None,
        ),
    ] {
        let (status, problem) = written(&app, Method::POST, "/handles", input.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{input} → {problem}");
        assert_eq!(problem["code"], code::CONFLICT.as_str(), "{problem}");
        assert_eq!(problem.get("errors").cloned(), named, "{input} → {problem}");
        let said = problem.to_string();
        for withheld in [
            "ada@probe.test",
            "ADA",
            HANDLES,
            "_email_key",
            "_slug_live",
            "23505",
        ] {
            assert!(!said.contains(withheld), "{withheld} in {said}");
        }
    }

    let id = second["id"].as_str().expect("the second handle's id");
    let (status, problem) = written(
        &app,
        Method::PUT,
        &format!("/handles/{id}"),
        json!({ "email": "ada@probe.test", "slug": "bob" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(problem["errors"], json!({ "email": unique }), "{problem}");
}

const BESPOKE: &str = "conflict_probe_bespoke";

mod bespoke {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Bespoke")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "conflict_probe_bespoke")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[derive(Clone, Debug, serde::Deserialize, schemars::JsonSchema, validator::Validate)]
struct CreateBespoke {
    email: String,
}

impl nest_rs_seaorm::CreateModel<bespoke::Entity> for CreateBespoke {
    fn into_active_model(self) -> bespoke::ActiveModel {
        bespoke::ActiveModel {
            id: sea_orm::Set(Uuid::now_v7()),
            email: sea_orm::Set(self.email),
        }
    }

    fn field_for(column: &str) -> Option<&'static str> {
        (column == "email").then_some("email")
    }
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct BespokeService;

impl nest_rs_seaorm::CrudService for BespokeService {
    type Entity = bespoke::Entity;
}

impl nest_rs_seaorm::Creatable for BespokeService {
    type Create = CreateBespoke;
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct EveryBespoke;

impl nest_rs_authz::AbilityFactory for EveryBespoke {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut nest_rs_authz::AbilityBuilder) {
        ab.can(nest_rs_authz::Action::Manage, bespoke::Entity);
    }
}

#[nest_rs_http::controller(path = "/bespoke")]
#[use_guards(
    crate::always_authenticated::AlwaysAuthenticated,
    nest_rs_authz::AbilityGuard<EveryBespoke>
)]
struct BespokeController {
    #[inject]
    svc: std::sync::Arc<BespokeService>,
}

#[nest_rs_http::crud(service = BespokeService, ops = [create])]
impl BespokeController {}

#[module(
    imports = [SeaOrmDatabaseModule],
    providers = [
        BespokeService,
        EveryBespoke,
        nest_rs_authz::AbilityGuard<EveryBespoke>,
        crate::always_authenticated::AlwaysAuthenticated,
        BespokeController,
    ],
)]
struct BespokeModule;

struct BespokeTables;

impl ProbeTables for BespokeTables {
    fn ddl() -> String {
        format!(
            "CREATE TABLE {BESPOKE} (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL CONSTRAINT {BESPOKE}_email_key UNIQUE
            )"
        )
    }
}

/// An entity declaring no input is read too: the input a service writes by hand
/// names its field through `field_for`.
#[tokio::test]
async fn a_hand_written_input_names_its_field_through_field_for() {
    use nest_rs_http::Method;
    use serde_json::json;

    let app = TestApp::builder()
        .module::<BespokeModule>()
        .ephemeral_database::<ProbeMigrator<BespokeTables>>()
        .build()
        .await
        .expect("the app boots on the probe database");
    let input = json!({ "email": "ada@probe.test" });

    let (status, first) = written(&app, Method::POST, "/bespoke", input.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, problem) = written(&app, Method::POST, "/bespoke", input).await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(
        problem["errors"],
        json!({ "email": [{ "code": "unique" }] }),
        "{problem}",
    );
}
