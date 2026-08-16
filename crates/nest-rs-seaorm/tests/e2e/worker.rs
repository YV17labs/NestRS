//! `WorkerDbContext` installs a live executor around a job so a
//! `#[scheduled]`/`#[processor]` queries through `Repo` without an injected
//! connection — and settles what that job wrote. Driven against the dev
//! Postgres, because the settling is the part no in-process double can show.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::Container;
use nest_rs_seaorm::{Executor, WorkerDbContext, current_executor};
use nest_rs_worker::{JobContext, JobSettlement, JobTransaction};
use sea_orm::{ConnectionTrait, Statement};

/// A table this file owns, dropped and recreated per test so a rollback is
/// observable as row count rather than inferred from a log.
async fn scratch_table(conn: &sea_orm::DatabaseConnection, name: &str) {
    for sql in [
        format!("DROP TABLE IF EXISTS {name}"),
        format!("CREATE TABLE {name} (id serial PRIMARY KEY)"),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .expect("the scratch table is created");
    }
}

async fn row_count(conn: &sea_orm::DatabaseConnection, name: &str) -> i64 {
    conn.query_one_raw(Statement::from_string(
        conn.get_database_backend(),
        format!("SELECT count(*)::bigint AS n FROM {name}"),
    ))
    .await
    .expect("the count query runs")
    .expect("count returns a row")
    .try_get::<i64>("", "n")
    .expect("the count is an integer")
}

fn context(conn: Arc<sea_orm::DatabaseConnection>) -> Arc<dyn JobContext> {
    let container = Container::builder().provide_arc(conn).build();
    Arc::new(WorkerDbContext::from_container(&container))
}

#[tokio::test]
async fn a_job_runs_in_a_transaction_by_default_and_its_writes_commit() {
    let conn = crate::harness::connect_arc().await;
    scratch_table(&conn, "worker_commit_probe").await;
    let ctx = context(conn.clone());

    assert!(
        current_executor().is_none(),
        "no ambient executor exists outside a job",
    );

    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        let executor = current_executor().expect("the job runs with an ambient executor installed");
        assert!(
            matches!(executor, Executor::Lazy(_)),
            "a worker job runs in a per-attempt transaction by default",
        );
        executor
            .execute_unprepared("INSERT INTO worker_commit_probe DEFAULT VALUES")
            .await
            .expect("the insert runs through the installed executor");
        true
    });
    assert_eq!(
        ctx.scope(JobTransaction::PerAttempt, job).await,
        JobSettlement::Settled
    );

    assert_eq!(
        row_count(&conn, "worker_commit_probe").await,
        1,
        "a job that returned Ok commits what it wrote",
    );
    assert!(
        current_executor().is_none(),
        "the ambient executor unwinds once the job completes",
    );
}

#[tokio::test]
async fn a_failed_job_leaves_nothing_for_the_retry_to_repeat() {
    let conn = crate::harness::connect_arc().await;
    scratch_table(&conn, "worker_rollback_probe").await;
    let ctx = context(conn.clone());

    // The whole point of the default: the job wrote, then failed. Before this,
    // the row survived and the retry inserted a second one.
    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        current_executor()
            .expect("ambient executor")
            .execute_unprepared("INSERT INTO worker_rollback_probe DEFAULT VALUES")
            .await
            .expect("the insert runs");
        false
    });
    assert_eq!(
        ctx.scope(JobTransaction::PerAttempt, job).await,
        JobSettlement::Settled
    );

    assert_eq!(
        row_count(&conn, "worker_rollback_probe").await,
        0,
        "a job that returned Err rolls back, so its retry starts from a clean slate",
    );
}

#[tokio::test]
async fn the_opt_out_runs_on_the_pool_and_each_statement_stands_alone() {
    let conn = crate::harness::connect_arc().await;
    scratch_table(&conn, "worker_pool_probe").await;
    let ctx = context(conn.clone());

    // `transactional = false` is what every job did before the default changed:
    // the write is already committed when the job goes on to fail.
    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        let executor = current_executor().expect("ambient executor");
        assert!(
            matches!(executor, Executor::Pool(_)),
            "the opt-out runs on the connection pool, with no transaction to settle",
        );
        executor
            .execute_unprepared("INSERT INTO worker_pool_probe DEFAULT VALUES")
            .await
            .expect("the insert runs");
        false
    });
    assert_eq!(
        ctx.scope(JobTransaction::Pool, job).await,
        JobSettlement::Settled
    );

    assert_eq!(
        row_count(&conn, "worker_pool_probe").await,
        1,
        "on the pool a write survives the failure that follows it — which is why \
         such a job owns its own idempotency",
    );
}

#[tokio::test]
async fn a_job_that_swallows_a_db_error_and_returns_ok_is_not_reported_as_succeeding() {
    let conn = crate::harness::connect_arc().await;
    scratch_table(&conn, "worker_poison_probe").await;
    conn.execute_unprepared("INSERT INTO worker_poison_probe (id) VALUES (1)")
        .await
        .expect("the row the job will collide with");
    let ctx = context(conn.clone());

    // The commonest job shape there is: loop, log what fails, carry on. Postgres
    // aborts the whole transaction on the first failed statement and then
    // *succeeds* the `COMMIT` while rolling back — so before the poison flag
    // this job reported `Ok`, wrote nothing, and said nothing about it.
    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        let executor = current_executor().expect("ambient executor");
        let collided = executor
            .execute_unprepared("INSERT INTO worker_poison_probe (id) VALUES (1)")
            .await;
        assert!(collided.is_err(), "the duplicate key must fail");
        // Swallowed, exactly as a real job would.
        let _ = executor
            .execute_unprepared("INSERT INTO worker_poison_probe (id) VALUES (2)")
            .await;
        true
    });

    let settlement = ctx.scope(JobTransaction::PerAttempt, job).await;
    let JobSettlement::Unhonoured(why) = settlement else {
        panic!("a job whose transaction was aborted cannot be reported as having succeeded");
    };
    assert!(
        !why.retryable,
        "and the duplicate key it collided with is a `23505`, which the next \
         attempt would hit again — so the budget buys nothing: {why}",
    );
    assert_eq!(
        row_count(&conn, "worker_poison_probe").await,
        1,
        "only the pre-seeded row survives — the attempt wrote nothing, which is \
         precisely why it must not be reported as a success",
    );
}

/// The failure that motivated classifying at all: a constraint checked at
/// `COMMIT`. Every statement succeeds, the job succeeds, and the commit is
/// refused — identically on every attempt, because the two rows the job writes
/// are what collide. Retrying it replays the whole body, side effects and all,
/// once per unit of retry budget, and dead-letters anyway.
#[tokio::test]
async fn a_commit_the_next_attempt_would_lose_too_is_not_retried() {
    let conn = crate::harness::connect_arc().await;
    for sql in [
        "DROP TABLE IF EXISTS worker_deferred_probe",
        "CREATE TABLE worker_deferred_probe (id serial PRIMARY KEY, slug text NOT NULL, \
         CONSTRAINT worker_deferred_probe_slug UNIQUE (slug) DEFERRABLE INITIALLY DEFERRED)",
    ] {
        conn.execute_unprepared(sql)
            .await
            .expect("the deferred-constraint probe table is created");
    }
    let ctx = context(conn.clone());

    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        let executor = current_executor().expect("ambient executor");
        for _ in 0..2 {
            executor
                .execute_unprepared("INSERT INTO worker_deferred_probe (slug) VALUES ('same')")
                .await
                .expect("a deferred constraint lets both statements through");
        }
        true
    });

    let JobSettlement::Unhonoured(why) = ctx.scope(JobTransaction::PerAttempt, job).await else {
        panic!("a commit that failed cannot be reported as a successful attempt");
    };
    assert!(
        !why.retryable,
        "a `23505` at commit is deterministic: the same body writes the same two \
         rows next time. Retrying replays every side effect the job performs \
         outside the transaction, then dead-letters regardless: {why}",
    );
    assert_eq!(
        row_count(&conn, "worker_deferred_probe").await,
        0,
        "and nothing landed, which is what makes the failure honest",
    );
}

/// The other half, and the reason "abort everything" is not the answer either:
/// a transient conflict. Two `SERIALIZABLE` transactions read each other's
/// predicate and then write into it; the second to commit is refused with
/// `40001`, and running the attempt again is exactly what clears it.
///
/// Postgres may raise the conflict at the write or at the `COMMIT` — which is
/// the point of reading its SQLSTATE rather than the site: the classification
/// is the same wherever it surfaces.
#[tokio::test]
async fn a_transaction_that_loses_a_serialization_conflict_stays_retryable() {
    let conn = crate::harness::connect_arc().await;
    for sql in [
        "DROP TABLE IF EXISTS worker_conflict_probe",
        "CREATE TABLE worker_conflict_probe (id serial PRIMARY KEY, class int NOT NULL)",
        "INSERT INTO worker_conflict_probe (class) VALUES (1), (2)",
    ] {
        conn.execute_unprepared(sql)
            .await
            .expect("the conflict probe table is created");
    }
    let ctx = context(conn.clone());
    let rival = crate::harness::connect().await;

    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async move {
        let executor = current_executor().expect("ambient executor");
        // First statement on the attempt's transaction, before any query has
        // fixed its snapshot — which is the only point Postgres accepts it.
        executor
            .execute_unprepared("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .await
            .expect("the attempt's transaction is raised to serializable");
        executor
            .execute_unprepared("SELECT count(*) FROM worker_conflict_probe WHERE class = 1")
            .await
            .expect("the read the rival is about to invalidate");

        // The rival runs to completion in the middle, so it is the first
        // committer and this attempt is the one refused.
        rival
            .execute_unprepared(
                "BEGIN; SET TRANSACTION ISOLATION LEVEL SERIALIZABLE; \
                 SELECT count(*) FROM worker_conflict_probe WHERE class = 2; \
                 INSERT INTO worker_conflict_probe (class) VALUES (1); COMMIT;",
            )
            .await
            .expect("the first committer wins");

        // Swallowed if it fails here rather than at the commit: either way the
        // attempt reported success and could not be honoured.
        let _ = executor
            .execute_unprepared("INSERT INTO worker_conflict_probe (class) VALUES (2)")
            .await;
        true
    });

    let JobSettlement::Unhonoured(why) = ctx.scope(JobTransaction::PerAttempt, job).await else {
        panic!("the losing transaction wrote nothing, so the attempt is a failure");
    };
    assert!(
        why.retryable,
        "a `40001` is what a retry budget is for — refusing to spend it here \
         would dead-letter work that the next attempt completes: {why}",
    );
}

/// A job whose `COMMIT` the database refuses.
///
/// The queue's whole promise is "an attempt that fails leaves nothing for the
/// retry to write again", and a commit-time constraint is where that promise is
/// either kept or silently broken: the job body succeeded, so a context that
/// reported the body's answer would ack a job that wrote nothing. The event is
/// also what carries `retryable`, and a schedule reports it while a queue spends
/// its budget on it — so getting it wrong either wastes a budget or drops work.
#[tokio::test]
async fn a_job_whose_commit_the_database_refuses_is_reported_as_unsettled() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = crate::harness::connect_arc().await;
    crate::harness::deferred_probe_tables(&conn, "worker_deferred").await;
    let ctx = context(conn.clone());

    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        let executor = current_executor().expect("the job runs with an ambient executor");
        executor
            .execute_unprepared(
                "INSERT INTO worker_deferred_children (id, parent_id) VALUES (1, 4242)",
            )
            .await
            .expect("a deferred constraint lets the statement through");
        // The job body itself succeeded. Everything after this is the boundary.
        true
    });

    let settlement = ctx.scope(JobTransaction::PerAttempt, job).await;
    let JobSettlement::Unhonoured(why) = settlement else {
        panic!("a job whose transaction never committed has not been settled");
    };
    assert!(
        !why.retryable,
        "a constraint that fails at commit fails identically forever — replaying \
         the body would buy nothing and repeat every side effect it had",
    );

    assert_eq!(
        row_count(&conn, "worker_deferred_children").await,
        0,
        "and nothing was written",
    );

    let event = logs.expect_one("nest_rs::orm", "job transaction commit failed");
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("retryable").as_deref(),
        Some("false"),
        "the same bit the settlement carries, so a log and a dead-letter cannot \
         disagree: {:?}",
        event.fields,
    );
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("worker_deferred")),
        "…and the database's own reason, got {:?}",
        event.fields,
    );
}
