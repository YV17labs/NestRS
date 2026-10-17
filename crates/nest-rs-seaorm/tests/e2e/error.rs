//! `src/error.rs` against Postgres: a refusal the server words around the input
//! it refused is said by its SQLSTATE and the names the server reported, and a
//! unique violation is a conflict — what no unit test can build, `PgDatabaseError`
//! having no public constructor.

use nest_rs_core::error_message;
use nest_rs_seaorm::{DbOutcome, ServiceError};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

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

    assert_eq!(DbOutcome::of(&refused), DbOutcome::Internal);
    let service = ServiceError::from(refused);
    assert!(matches!(service, ServiceError::Db(_)), "{service:?}");
    assert!(!error_message(&service).contains("secret-123"));
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

    assert_eq!(DbOutcome::of(&taken), DbOutcome::Conflict);
    let said = error_message(&taken);
    assert_eq!(
        said,
        format!(
            "Execution Error: database error 23505 on constraint \"{UNIQUE}\", table \"{TABLE}\""
        ),
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

    assert_eq!(DbOutcome::of(&missing), DbOutcome::Internal);
    let said = error_message(&missing);
    assert!(
        said.starts_with("Execution Error: database error 23502 on "),
        "{said}"
    );
    for named in [format!("table \"{TABLE}\""), "column \"email\"".to_owned()] {
        assert!(said.contains(&named), "{named} in {said}");
    }
}
