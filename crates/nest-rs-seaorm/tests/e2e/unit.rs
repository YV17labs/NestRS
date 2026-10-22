//! The HTTP edge's unit of work over the binding: a mutating request runs in
//! one lazy transaction, committed on a success and rolled back on every
//! failure however it was answered, and a transaction the request leaked or
//! could not honour fails it with a `500`.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{
    Caught, ExceptionFilter, Layer, UnitContext, async_trait, exception_filter, injectable, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{HttpConfig, HttpError, HttpModule, HttpView, StatusCode};
use nest_rs_seaorm::{
    Executor, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, current_executor,
};
use nest_rs_testing::TestApp;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::harness::{
    ProbeController, ProbeHandler, ProbeModule, answered, committed_rows, fresh_table, probe,
    probe_app,
};

/// Write one row into `table` on the request's executor.
async fn insert_into(table: &str) {
    let inserted = current_executor()
        .expect("the handler runs with an ambient executor")
        .execute_unprepared(&format!("INSERT INTO {table} (id) VALUES (1)"))
        .await
        .expect("the insert runs inside the request transaction");
    assert_eq!(inserted.rows_affected(), 1, "the write really lands");
}

#[tokio::test]
async fn an_escaped_transaction_fails_an_otherwise_successful_response() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::OK, || async {
        let escaped = current_executor().expect("the handler runs with an ambient executor");
        tokio::spawn(async move {
            let _hold = escaped;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
    }))
    .await;

    assert_eq!(
        probe(&app).await,
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
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::CREATED, || async {
        current_executor().expect("the handler runs with an ambient executor");
    }))
    .await;

    assert_eq!(probe(&app).await, StatusCode::CREATED);
}

#[tokio::test]
async fn a_writing_handler_commits_through_the_lazy_transaction() {
    const TABLE: &str = "lazy_commit_probe";
    let conn = crate::harness::connect().await;
    fresh_table(&conn, TABLE).await;
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::CREATED, || {
        insert_into(TABLE)
    }))
    .await;

    assert_eq!(probe(&app).await, StatusCode::CREATED);
    assert_eq!(
        committed_rows(&conn, TABLE).await,
        1,
        "the lazily opened transaction must commit",
    );
}

/// Answers every failure `200`, as an envelope a product shapes errors with.
#[injectable]
#[derive(Default)]
struct AnswersOk;

impl Layer for AnswersOk {}

#[async_trait]
impl ExceptionFilter for AnswersOk {
    type Exception = dyn Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        match HttpView::of(cx) {
            Ok(view) => Caught::Answered(view.respond(answered(StatusCode::OK))),
            Err(_) => Caught::Declined,
        }
    }
}

#[module(imports = [ProbeModule], providers = [AnswersOk])]
struct FilteredProbeModule;

#[tokio::test]
async fn a_handler_error_a_filter_answered_rolls_the_handlers_writes_back() {
    const TABLE: &str = "filtered_rollback_probe";
    let conn = crate::harness::connect().await;
    fresh_table(&conn, TABLE).await;
    let app = TestApp::builder()
        .provide(ProbeHandler::new(|| async {
            insert_into(TABLE).await;
            Err(HttpError::new(
                std::io::Error::other("refused after writing"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }))
        .module::<FilteredProbeModule>()
        .use_exception_filters_global([exception_filter::<AnswersOk>()])
        .build()
        .await
        .expect("the probe app boots against live Postgres");

    assert_eq!(
        probe(&app).await,
        StatusCode::OK,
        "the filter's answer is what the client reads",
    );
    assert_eq!(
        committed_rows(&conn, TABLE).await,
        0,
        "a failed unit never commits, however a filter answered it",
    );
}

#[tokio::test]
async fn a_handler_answering_not_found_rolls_its_writes_back() {
    const TABLE: &str = "not_found_rollback_probe";
    let conn = crate::harness::connect().await;
    fresh_table(&conn, TABLE).await;
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::NOT_FOUND, || {
        insert_into(TABLE)
    }))
    .await;

    assert_eq!(probe(&app).await, StatusCode::NOT_FOUND);
    assert_eq!(
        committed_rows(&conn, TABLE).await,
        0,
        "a 4xx the handler answered on purpose still rolls back",
    );
}

/// Whether the refusing guard ran inside the request's unit, its transaction
/// not yet opened, and whether the handler ran after it.
static GUARD_SAW_UNOPENED_UNIT: AtomicBool = AtomicBool::new(false);
static HANDLER_RAN: AtomicBool = AtomicBool::new(false);

/// Refuses every request, after reading the executor it runs under.
#[injectable]
#[derive(Default)]
struct Refusing;

impl Layer for Refusing {}

#[async_trait]
impl Guard for Refusing {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let unopened =
            matches!(current_executor(), Some(Executor::Lazy(lazy)) if !lazy.is_opened());
        GUARD_SAW_UNOPENED_UNIT.store(unopened, Ordering::SeqCst);
        Err(Denial::forbidden("refused before the handler"))
    }
}

impl GuardFor<HttpView> for Refusing {}

#[module(imports = [ProbeModule], providers = [Refusing])]
struct RefusingProbeModule;

#[tokio::test]
async fn a_denied_mutating_request_opens_no_transaction() {
    let app = TestApp::builder()
        .provide(ProbeHandler::answering(StatusCode::CREATED, || async {
            HANDLER_RAN.store(true, Ordering::SeqCst);
        }))
        .module::<RefusingProbeModule>()
        .use_guards_global([guard::<Refusing>()])
        .build()
        .await
        .expect("the probe app boots against live Postgres");

    assert_eq!(probe(&app).await, StatusCode::FORBIDDEN);
    assert!(
        GUARD_SAW_UNOPENED_UNIT.load(Ordering::SeqCst),
        "the guard runs inside the request's lazy unit, before anything opened it",
    );
    assert!(
        !HANDLER_RAN.load(Ordering::SeqCst),
        "and nothing after the refusal touches the data layer",
    );
}

/// A handler that swallows a `DbErr` and answers `200`: Postgres aborts the
/// transaction on the first failure, so its `COMMIT` succeeds having written
/// nothing.
#[tokio::test]
async fn a_swallowed_statement_failure_refuses_the_success_it_was_told_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::OK, || async {
        let failed = current_executor()
            .expect("the handler runs with an ambient executor")
            .execute_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "INSERT INTO a_table_this_test_never_created VALUES (1)",
            ))
            .await;
        assert!(failed.is_err(), "the statement really did fail");
    }))
    .await;

    assert_eq!(
        probe(&app).await,
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
        "which edge swallowed it, since every edge settles through this one seam: {:?}",
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
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::OK, || async {
        current_executor()
            .expect("the handler runs with an ambient executor")
            .execute_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT 1",
            ))
            .await
            .expect("a statement that works");
    }))
    .await;

    assert_eq!(probe(&app).await, StatusCode::OK);
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
#[tokio::test]
async fn a_commit_the_database_refuses_fails_the_response_it_had_already_built() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect().await;
    crate::harness::deferred_probe_tables(&conn, "commit_probe").await;
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::OK, || async {
        current_executor()
            .expect("the handler runs with an ambient executor")
            .execute_unprepared(
                "INSERT INTO commit_probe_children (id, parent_id) VALUES (1, 4242)",
            )
            .await
            .expect("a deferred constraint lets the statement through — that is the point");
    }))
    .await;

    assert_eq!(
        probe(&app).await,
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
    let app =
        probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::BAD_REQUEST, || async {
            current_executor()
                .expect("the handler runs with an ambient executor")
                .execute_unprepared("SELECT 1")
                .await
                .expect("a statement opens the lazy transaction");
            kill_this_transactions_backend().await;
        }))
        .await;

    assert_eq!(
        probe(&app).await,
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
    let app = probe_app::<ProbeModule>(ProbeHandler::answering(StatusCode::OK, || async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        // Read first: a poisoned transaction refuses every later statement.
        let pid = crate::harness::backend_pid(&executor).await;
        let failed = executor
            .execute_unprepared("INSERT INTO a_table_this_test_never_created VALUES (1)")
            .await;
        assert!(failed.is_err(), "the statement poisons the transaction");
        let killer = crate::harness::connect().await;
        crate::harness::terminate_backend(&killer, pid).await;
    }))
    .await;

    assert_eq!(
        probe(&app).await,
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

/// The probe route over a binding that observes serialization conflicts.
#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            observe_serialization_conflicts: true,
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
    ],
    providers = [ProbeController],
)]
struct ConflictProbeModule;

/// The textbook SSI conflict: each request reads the rows the other writes, so
/// Postgres refuses one of them at `COMMIT`.
fn racing_writes() -> ProbeHandler {
    let calls = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(tokio::sync::Barrier::new(2));
    let writes = Arc::new(tokio::sync::Barrier::new(2));
    ProbeHandler::answering(StatusCode::OK, move || {
        let (class, reads, writes) = (
            calls.fetch_add(1, Ordering::SeqCst),
            Arc::clone(&reads),
            Arc::clone(&writes),
        );
        let (read, write) = if class == 0 { (1, 2) } else { (2, 1) };
        async move {
            let executor = current_executor().expect("the handler runs with an ambient executor");
            // Postgres accepts it only as the first statement; `BEGIN` is lazy.
            executor
                .execute_unprepared("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .await
                .expect("the isolation level is set on a fresh transaction");
            executor
                .execute_unprepared(&format!(
                    "SELECT count(*) FROM serialization_probe WHERE class = {read}"
                ))
                .await
                .expect("the read that makes the two transactions overlap");
            // Both sides must have read before either writes.
            reads.wait().await;
            executor
                .execute_unprepared(&format!(
                    "INSERT INTO serialization_probe (class) VALUES ({write})"
                ))
                .await
                .expect("the write the other side's read did not see");
            // Both must have written before either commits, or SSI may cancel
            // the loser at its `INSERT` instead of at `COMMIT`.
            writes.wait().await;
        }
    })
}

#[tokio::test]
async fn a_commit_another_transaction_won_is_a_conflict_and_not_an_outage() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect().await;
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
    let app = probe_app::<ConflictProbeModule>(racing_writes()).await;

    let (left, right) = tokio::join!(probe(&app), probe(&app));

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

/// A handler writing to `table` and holding work for the commit that records
/// what it sees, answering `status`.
fn writes_and_holds(
    table: &'static str,
    observer: Arc<DatabaseConnection>,
    record: crate::harness::Sightings,
    status: StatusCode,
) -> ProbeHandler {
    ProbeHandler::answering(status, move || {
        crate::harness::write_and_hold(table, observer.clone(), record.clone())
    })
}

/// Work held for the commit of a mutating request runs after it — on the pool,
/// seeing the row the request committed — and before the response leaves.
#[tokio::test]
async fn work_held_for_the_commit_runs_once_the_request_committed() {
    use crate::harness::{Sighting, Sightings, sighted};
    const TABLE: &str = "after_commit_http_commit";
    let conn = crate::harness::connect_arc().await;
    fresh_table(&conn, TABLE).await;
    let sightings = Sightings::default();
    let app = probe_app::<ProbeModule>(writes_and_holds(
        TABLE,
        conn.clone(),
        sightings.clone(),
        StatusCode::CREATED,
    ))
    .await;

    assert_eq!(probe(&app).await, StatusCode::CREATED);
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
    use crate::harness::{Sightings, sighted};
    const TABLE: &str = "after_commit_http_rollback";
    let conn = crate::harness::connect_arc().await;
    fresh_table(&conn, TABLE).await;
    let sightings = Sightings::default();
    let app = probe_app::<ProbeModule>(writes_and_holds(
        TABLE,
        conn.clone(),
        sightings.clone(),
        StatusCode::UNPROCESSABLE_ENTITY,
    ))
    .await;

    assert_eq!(probe(&app).await, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(committed_rows(&conn, TABLE).await, 0);
    assert!(
        sighted(&sightings).is_empty(),
        "nothing reacts to a write that never landed",
    );
}

/// The probe route under a one-second request timeout.
#[module(
    imports = [
        HttpModule::for_root(HttpConfig {
            request_timeout: Some(Duration::from_secs(1)),
            ..HttpConfig::default()
        }),
        ProbeModule,
    ],
)]
struct DeadlineProbeModule;

/// How many times the listener on the request's committed write ran.
static LISTENED: AtomicU32 = AtomicU32::new(0);

/// The test's word to that listener, given once the response is in.
static RELEASED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The request timeout bounds the handler: one that wrote and answered within
/// it answers its own status, never `503`, however long what listens to its
/// commit runs — the response waits for that until the deadline at most, then
/// lets it run on. The listener waits for the test's word, so a response it
/// held could only be the net's `500`, whatever the commit took.
#[tokio::test]
async fn a_post_whose_listener_outlives_the_request_timeout_answers_its_own_status() {
    const TABLE: &str = "deadline_commit_probe";
    let conn = crate::harness::connect().await;
    fresh_table(&conn, TABLE).await;
    let app =
        probe_app::<DeadlineProbeModule>(ProbeHandler::answering(StatusCode::CREATED, || async {
            insert_into(TABLE).await;
            nest_rs_database::after_commit(async {
                RELEASED.notified().await;
                LISTENED.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        }))
        .await;

    assert_eq!(
        probe(&app).await,
        StatusCode::CREATED,
        "a write that landed is never a 503, nor held by its listener"
    );
    assert_eq!(committed_rows(&conn, TABLE).await, 1);
    RELEASED.notify_one();
    nest_rs_testing::wait_until(Duration::from_secs(5), || {
        LISTENED.load(Ordering::SeqCst) == 1
    })
    .await;
}
