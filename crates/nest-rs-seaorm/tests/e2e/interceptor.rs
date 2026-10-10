//! `DbContext` opens a real transaction around mutating handlers: commits on
//! 2xx/3xx, rolls back on anything else, and fails a leaked executor with a 500.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_interceptors::InterceptorExt;
use nest_rs_seaorm::{DbContext, SeaOrmConfig, current_executor};
use poem::endpoint::make;
use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Request, Response, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

fn config() -> Arc<SeaOrmConfig> {
    Arc::new(SeaOrmConfig::default())
}

fn mutating_request() -> Request {
    Request::builder()
        .method(Method::POST)
        .uri("/".parse().unwrap())
        .finish()
}

fn status_of(result: Result<Response>) -> StatusCode {
    match result {
        Ok(resp) => resp.status(),
        Err(err) => err.into_response().status(),
    }
}

#[tokio::test]
async fn an_escaped_transaction_fails_an_otherwise_successful_response() {
    let logs = nest_rs_testing::LogCapture::install();
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let escaped = current_executor().expect("the handler runs with an ambient executor");
        tokio::spawn(async move {
            let _hold = escaped;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        StatusCode::OK.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a leaked transaction must surface as a 500, never a false 2xx",
    );

    // The `500` is opaque: the event alone names the cause.
    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "executor escaped into a spawned task",
    );
    assert_eq!(event.level, "error");
    assert_eq!(event.field("outcome").as_deref(), Some("rollback_and_fail"));
    assert_eq!(
        event.field("transport").as_deref(),
        Some("http"),
        "the event names the edge the leak happened on, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_well_behaved_mutating_handler_keeps_its_status() {
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        current_executor().expect("the handler runs with an ambient executor");
        StatusCode::CREATED.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_mapped_error_2xx_rolls_back_the_handlers_writes() {
    let conn = crate::harness::connect_arc().await;

    conn.execute_unprepared("DROP TABLE IF EXISTS mapped_rollback_probe")
        .await
        .expect("drop any leftover probe table");
    conn.execute_unprepared("CREATE TABLE mapped_rollback_probe (id INT PRIMARY KEY)")
        .await
        .expect("create the probe table");

    let ctx = DbContext::new(conn.clone(), config());

    // A 2xx tagged `MappedError`, as a route-site Filter emits: the mapping
    // shapes the answer, it does not bless the failed handler's writes.
    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        let inserted = executor
            .execute_unprepared("INSERT INTO mapped_rollback_probe (id) VALUES (1)")
            .await
            .expect("the insert runs inside the request transaction");
        assert_eq!(
            inserted.rows_affected(),
            1,
            "the write really lands inside the open transaction",
        );

        let mut resp = StatusCode::OK.into_response();
        resp.extensions_mut().insert(nest_rs_http::MappedError);
        resp
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::OK,
        "the mapped success status is still returned to the client",
    );

    let remaining: i32 = conn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::int AS n FROM mapped_rollback_probe",
        ))
        .await
        .expect("count on the pool")
        .expect("count returns a row")
        .try_get("", "n")
        .expect("read the count");
    assert_eq!(
        remaining, 0,
        "a MappedError-tagged 2xx must roll back the handler's writes",
    );

    conn.execute_unprepared("DROP TABLE IF EXISTS mapped_rollback_probe")
        .await
        .expect("clean up the probe table");
}

/// A handler that swallows a `DbErr` and answers `200`: Postgres aborts the
/// transaction on the first failure, so its `COMMIT` succeeds having written
/// nothing.
#[tokio::test]
async fn a_swallowed_statement_failure_refuses_the_success_it_was_told_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        let failed = executor
            .execute_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "INSERT INTO a_table_this_test_never_created VALUES (1)",
            ))
            .await;
        assert!(failed.is_err(), "the statement really did fail");
        StatusCode::OK.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a boundary that cannot honour the success it was handed must not report one",
    );

    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "a statement failed inside this transaction but the boundary reported success; \
         nothing it wrote could be committed",
    );
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("transport").as_deref(),
        Some("http"),
        "which edge swallowed it, since all four settle through this one seam: {:?}",
        event.fields,
    );
    // A missing table never heals: a retry would only replay side effects.
    assert_eq!(
        event.field("retryable").as_deref(),
        Some("false"),
        "{:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_clean_mutation_commits_without_any_of_that() {
    let logs = nest_rs_testing::LogCapture::install();
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        executor
            .execute_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT 1",
            ))
            .await
            .expect("a statement that works");
        StatusCode::OK.into_response()
    });

    assert_eq!(
        status_of(endpoint.interceptor(ctx).call(mutating_request()).await),
        StatusCode::OK,
    );
    assert!(
        logs.events()
            .iter()
            .all(|e| !e.message.contains("boundary reported success")),
        "{:#?}",
        logs.events(),
    );
}

/// A `COMMIT` that fails on a deferred foreign key, after every statement
/// succeeded and the handler returned `200`.
async fn deferred_constraint_tables() -> Arc<sea_orm::DatabaseConnection> {
    let conn = crate::harness::connect_arc().await;
    crate::harness::deferred_probe_tables(&conn, "commit_probe").await;
    conn
}

#[tokio::test]
async fn a_commit_the_database_refuses_fails_the_response_it_had_already_built() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = deferred_constraint_tables().await;
    let ctx = DbContext::new(Arc::clone(&conn), config());

    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        executor
            .execute_unprepared(
                "INSERT INTO commit_probe_children (id, parent_id) VALUES (1, 4242)",
            )
            .await
            .expect("a deferred constraint lets the statement through — that is the point");
        StatusCode::OK.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a response whose transaction did not commit must not go out as a success",
    );

    let landed: i32 = conn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::int AS n FROM commit_probe_children WHERE id = 1",
        ))
        .await
        .expect("count on the pool")
        .expect("count returns a row")
        .try_get("", "n")
        .expect("read the count");
    assert_eq!(landed, 0, "and nothing was written");

    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "transaction commit failed");
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("commit_probe")),
        "the event carries the database's own reason — the only copy, since the \
         500 is opaque, got {:?}",
        event.fields,
    );
}

/// Terminate the backend serving the ambient executor, from a second
/// connection — so the pending `ROLLBACK` has no session left to reach.
async fn kill_this_transactions_backend() {
    let executor = current_executor().expect("the handler runs with an ambient executor");
    let pid = crate::harness::backend_pid(&executor).await;
    let killer = crate::harness::connect().await;
    crate::harness::terminate_backend(&killer, pid).await;
}

#[tokio::test]
async fn a_rollback_with_no_session_left_to_reach_is_reported_rather_than_assumed() {
    let logs = nest_rs_testing::LogCapture::install();
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        executor
            .execute_unprepared("SELECT 1")
            .await
            .expect("a statement opens the lazy transaction");
        kill_this_transactions_backend().await;
        StatusCode::BAD_REQUEST.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the handler's own status still reaches the client — a rollback that \
         could not be issued does not change what the request answered",
    );

    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "transaction rollback failed");
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the event carries why the rollback could not be issued, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_poisoned_rollback_that_cannot_be_issued_is_its_own_line() {
    let logs = nest_rs_testing::LogCapture::install();
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        // Read first: a poisoned transaction refuses every later statement.
        let pid = crate::harness::backend_pid(&executor).await;
        let failed = executor
            .execute_unprepared("INSERT INTO a_table_this_test_never_created VALUES (1)")
            .await;
        assert!(failed.is_err(), "the statement poisons the transaction");
        let killer = crate::harness::connect().await;
        crate::harness::terminate_backend(&killer, pid).await;
        StatusCode::OK.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a swallowed statement failure still refuses the success it was handed",
    );

    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "poisoned transaction rollback failed",
    );
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the event carries why the rollback could not be issued, got {:?}",
        event.fields,
    );
    assert!(
        logs.find(nest_rs_seaorm::target::ORM, "transaction rollback failed")
            .is_empty(),
        "and it is *this* line, not the failing-handler one: {:#?}",
        logs.events(),
    );
}

fn conflict_observing_config() -> Arc<SeaOrmConfig> {
    Arc::new(SeaOrmConfig {
        observe_serialization_conflicts: true,
        ..SeaOrmConfig::default()
    })
}

/// The textbook SSI conflict: each side reads the rows the other writes, so
/// Postgres refuses one of them at `COMMIT`.
async fn racing_write(
    conn: Arc<sea_orm::DatabaseConnection>,
    read_barrier: Arc<tokio::sync::Barrier>,
    write_barrier: Arc<tokio::sync::Barrier>,
    reads: i32,
    writes: i32,
) -> StatusCode {
    let ctx = DbContext::new(conn, conflict_observing_config());
    let endpoint = make(move |_req: Request| {
        let read_barrier = Arc::clone(&read_barrier);
        let write_barrier = Arc::clone(&write_barrier);
        async move {
            let executor = current_executor().expect("the handler runs with an ambient executor");
            // Postgres accepts it only as the first statement; `BEGIN` is lazy.
            executor
                .execute_unprepared("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .await
                .expect("the isolation level is set on a fresh transaction");
            executor
                .execute_unprepared(&format!(
                    "SELECT count(*) FROM serialization_probe WHERE class = {reads}"
                ))
                .await
                .expect("the read that makes the two transactions overlap");
            // Both sides must have read before either writes.
            read_barrier.wait().await;
            executor
                .execute_unprepared(&format!(
                    "INSERT INTO serialization_probe (class) VALUES ({writes})"
                ))
                .await
                .expect("the write the other side's read did not see");
            // Both must have written before either commits, or SSI may cancel
            // the loser at its `INSERT` instead of at `COMMIT`.
            write_barrier.wait().await;
            StatusCode::OK.into_response()
        }
    });
    status_of(endpoint.interceptor(ctx).call(mutating_request()).await)
}

#[tokio::test]
async fn a_commit_another_transaction_won_is_a_conflict_and_not_an_outage() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect_arc().await;
    crate::harness::setup_shared_table(
        &conn,
        "serialization_probe",
        "CREATE TABLE IF NOT EXISTS serialization_probe (
             id SERIAL PRIMARY KEY, class INT NOT NULL
         );
         INSERT INTO serialization_probe (class)
             SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM serialization_probe);
         INSERT INTO serialization_probe (class)
             SELECT 2 WHERE NOT EXISTS (SELECT 1 FROM serialization_probe WHERE class = 2);",
    )
    .await;

    let reads = Arc::new(tokio::sync::Barrier::new(2));
    let writes = Arc::new(tokio::sync::Barrier::new(2));
    let (left, right) = tokio::join!(
        racing_write(
            Arc::clone(&conn),
            Arc::clone(&reads),
            Arc::clone(&writes),
            1,
            2
        ),
        racing_write(
            Arc::clone(&conn),
            Arc::clone(&reads),
            Arc::clone(&writes),
            2,
            1
        ),
    );

    // Which side loses is the database's choice: assert on the pair.
    let refused = [left, right]
        .into_iter()
        .filter(|status| *status == StatusCode::INTERNAL_SERVER_ERROR)
        .count();
    assert_eq!(
        refused, 1,
        "SERIALIZABLE lets one through and refuses the other, got {left} and {right}",
    );

    let event = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "serialization conflict at commit",
    );
    assert_eq!(
        event.level, "warn",
        "a conflict is the isolation level working, not an incident — filed at \
         `error` it would drown the failures that are",
    );
    assert!(
        event
            .field("hint")
            .is_some_and(|h| h.contains("retry_on_conflict")),
        "the line carries where a retry *can* be written, since the one place \
         it cannot is here, got {:?}",
        event.fields,
    );
    assert!(
        logs.find(nest_rs_seaorm::target::ORM, "transaction commit failed")
            .is_empty(),
        "and it is not also filed as a plain commit failure: {:#?}",
        logs.events(),
    );
}

/// Work held for the commit of a mutating request runs after it — on the pool,
/// seeing the row the request committed — and before the response leaves.
#[tokio::test]
async fn work_held_for_the_commit_runs_once_the_request_committed() {
    use crate::harness::{Sighting, Sightings, after_commit_table, sighted, write_and_hold};
    const TABLE: &str = "after_commit_http_commit";
    let conn = crate::harness::connect_arc().await;
    after_commit_table(&conn, TABLE).await;
    let sightings = Sightings::default();
    let ctx = DbContext::new(conn.clone(), config());

    let (observer, record) = (conn.clone(), sightings.clone());
    let endpoint = make(move |_req: Request| {
        let (observer, record) = (observer.clone(), record.clone());
        async move {
            write_and_hold(TABLE, observer, record).await;
            StatusCode::CREATED.into_response()
        }
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        sighted(&sightings),
        [Sighting {
            committed: 1,
            on_pool: true
        }],
        "the work ran once, after the commit, outside the transaction",
    );
}

/// A request that fails rolls its writes back, and the work that waited for
/// their commit never runs.
#[tokio::test]
async fn work_held_by_a_request_that_rolled_back_never_runs() {
    use crate::harness::{Sightings, after_commit_table, committed_rows, sighted, write_and_hold};
    const TABLE: &str = "after_commit_http_rollback";
    let conn = crate::harness::connect_arc().await;
    after_commit_table(&conn, TABLE).await;
    let sightings = Sightings::default();
    let ctx = DbContext::new(conn.clone(), config());

    let (observer, record) = (conn.clone(), sightings.clone());
    let endpoint = make(move |_req: Request| {
        let (observer, record) = (observer.clone(), record.clone());
        async move {
            write_and_hold(TABLE, observer, record).await;
            StatusCode::UNPROCESSABLE_ENTITY.into_response()
        }
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(committed_rows(&conn, TABLE).await, 0);
    assert!(
        sighted(&sightings).is_empty(),
        "nothing reacts to a write that never landed",
    );
}
