//! `DbContext` opens a real transaction around mutating handlers — commits on
//! 2xx/3xx, rolls back on anything else, and surfaces a leaked executor as a
//! loud 500 (silent rollback of a "successful" mutation is data loss).

use std::sync::Arc;
use std::time::Duration;

use nest_rs_interceptors::InterceptorExt;
use nest_rs_seaorm::{DbContext, SeaOrmDatabaseConfig, current_executor};
use poem::endpoint::make;
use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Request, Response, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

fn config() -> Arc<SeaOrmDatabaseConfig> {
    Arc::new(SeaOrmDatabaseConfig::default())
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

    // The `500` is an opaque problem+json, so from the outside a leaked
    // executor is indistinguishable from any other internal error — including
    // the ordinary bug the developer will look for first. The event is what
    // names the actual cause, and `outcome` is what says whether the boundary
    // rolled back or rolled back *and* failed a response it had already built.
    let event = logs.expect_one("nest_rs::orm", "executor escaped into a spawned task");
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

    // A committed scratch table on the pool, isolated from the request txn.
    conn.execute_unprepared("DROP TABLE IF EXISTS mapped_rollback_probe")
        .await
        .expect("drop any leftover probe table");
    conn.execute_unprepared("CREATE TABLE mapped_rollback_probe (id INT PRIMARY KEY)")
        .await
        .expect("create the probe table");

    let ctx = DbContext::new(conn.clone(), config());

    // A handler that writes inside the request transaction, then hands back a
    // 2xx tagged `MappedError` — exactly what a route-site Filter emits after
    // mapping the handler's `Err`. `DbContext` must roll back regardless of the
    // success status: the mapping shapes the client answer, it does not bless
    // the failed handler's writes.
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

    // The pool sees the committed, empty table: the tagged 2xx rolled the insert
    // back rather than committing it behind a success status.
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

/// A handler that runs a statement, gets a `DbErr`, ignores it, and answers
/// `200`.
///
/// This is the one combination that loses writes in silence: Postgres aborts a
/// transaction on the first failed statement, so every statement after it fails
/// too and the eventual `COMMIT` *succeeds* having written nothing. A boundary
/// that trusted the response would have committed nothing and reported
/// success — the client is told the write landed, the row is not there, and no
/// error exists anywhere.
///
/// The `?`-shaped handler never reaches this: a propagated `DbErr` is a 500 and
/// the boundary rolls back. What reaches it is a handler that swallows — a
/// `let _ =`, a `.ok()`, a `match` whose error arm logs and carries on — which
/// is ordinary defensive code everywhere except inside a transaction.
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
        // Swallowed, deliberately — and then a success is reported anyway.
        StatusCode::OK.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a boundary that cannot honour the success it was handed must not report one",
    );

    let event = logs.expect_one(
        "nest_rs::orm",
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
    // Whether repeating the request could ever end differently. A missing table
    // never will, so spending a retry budget on it buys nothing and replays
    // every non-transactional side effect the handler had.
    assert_eq!(
        event.field("retryable").as_deref(),
        Some("false"),
        "{:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_clean_mutation_commits_without_any_of_that() {
    // The other direction: every successful mutation goes through the same
    // settle, so a check reading the flag wrongly would fail all of them.
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

/// A `COMMIT` that fails on a constraint Postgres checks *at commit time*.
///
/// This is the branch the whole boundary exists for and the hardest to believe
/// without seeing it: every statement in the handler succeeded, the handler
/// returned `200`, and the write still did not land. A deferred foreign key is
/// the plainest way to produce it — the standard's own mechanism for "check
/// this at the end of the transaction" — and a row inserted against a parent
/// that never arrives is exactly the shape it exists to catch.
///
/// Without the boundary refusing here, the client is told `200` and the row is
/// not there, with nothing anywhere to say so.
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

    let event = logs.expect_one("nest_rs::orm", "transaction commit failed");
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

// --- when the rollback itself cannot be issued -------------------------------
//
// Both branches below share one situation: the boundary decided to roll back
// and the connection was already gone. Postgres has ended the transaction
// itself in that case, so nothing is left half-applied — but the boundary
// cannot *know* that, and an operator reading "rolled back" about a session
// that died has been told something the framework did not verify. Hence a line
// per branch, and two branches because they answer different questions: one is
// a handler that failed, the other a handler that reported success over a
// failed statement.

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
        // A failing handler: the boundary is going to roll back.
        StatusCode::BAD_REQUEST.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the handler's own status still reaches the client — a rollback that \
         could not be issued does not change what the request answered",
    );

    let event = logs.expect_one("nest_rs::orm", "transaction rollback failed");
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the event carries why the rollback could not be issued, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_poisoned_rollback_that_cannot_be_issued_is_its_own_line() {
    // Its twin, and the reason they are two messages rather than one: here the
    // handler reported *success*. An operator seeing only "rollback failed"
    // would look for the error the handler returned, and there was none.
    let logs = nest_rs_testing::LogCapture::install();
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        // The pid is read *first*: a poisoned transaction refuses every
        // subsequent statement, this one included.
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

    let event = logs.expect_one("nest_rs::orm", "poisoned transaction rollback failed");
    assert_eq!(event.level, "error");
    assert!(
        event.field("error").is_some(),
        "the event carries why the rollback could not be issued, got {:?}",
        event.fields,
    );
    assert!(
        logs.find("nest_rs::orm", "transaction rollback failed")
            .is_empty(),
        "and it is *this* line, not the failing-handler one: {:#?}",
        logs.events(),
    );
}

// --- a commit the database refuses because another transaction won ------------
//
// Under SERIALIZABLE, Postgres lets two transactions run to completion and then
// refuses one of them at `COMMIT` with `40001` — the whole point of the
// isolation level. That is not a bug in the app and not an outage: it is the
// database asking for the work to be done again, and a deployment that runs
// SERIALIZABLE sees it under normal load.
//
// So it is a `warn` with a remedy rather than an `error`: the interceptor
// cannot retry (a handler body is not replayable from outside it), and an
// operator staring at `error`-level "commit failed" lines would be hunting an
// incident that is really a tuning signal. `observe_serialization_conflicts`
// is the switch, off by default, because a deployment on READ COMMITTED never
// sees one and does not want the branch.

fn conflict_observing_config() -> Arc<SeaOrmDatabaseConfig> {
    Arc::new(SeaOrmDatabaseConfig {
        observe_serialization_conflicts: true,
        ..SeaOrmDatabaseConfig::default()
    })
}

/// The textbook SSI conflict: each side reads the rows the *other* is about to
/// write. Neither read sees the other's insert, so both would be serializable
/// only in an order that does not exist — and Postgres finds that out at
/// `COMMIT`.
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
            // First statement in the transaction, which is where Postgres
            // accepts it: `BEGIN` is issued lazily just before this.
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
            // And both must have *written* before either commits. Without this
            // the test was flaky at ~21%: SSI can cancel the loser eagerly, at
            // its `INSERT`, once the winner has already committed — a correct
            // `40001`, raised at the statement instead of at `COMMIT`, which is
            // a different branch of the interceptor and made the `.expect`
            // above panic. Which side loses is still the database's choice; all
            // this pins is that neither has committed when the other writes, so
            // the conflict has nowhere to surface but the commit.
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

    // Which side loses is the database's choice, so the assertion is on the
    // pair: exactly one of them must have been refused.
    let refused = [left, right]
        .into_iter()
        .filter(|status| *status == StatusCode::INTERNAL_SERVER_ERROR)
        .count();
    assert_eq!(
        refused, 1,
        "SERIALIZABLE lets one through and refuses the other, got {left} and {right}",
    );

    let event = logs.expect_one("nest_rs::orm", "serialization conflict at commit");
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
        logs.find("nest_rs::orm", "transaction commit failed")
            .is_empty(),
        "and it is not also filed as a plain commit failure: {:#?}",
        logs.events(),
    );
}
