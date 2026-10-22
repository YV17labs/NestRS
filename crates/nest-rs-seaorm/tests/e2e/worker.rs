//! `SeaOrmUnitOfWork` installs a live executor around a job's unit of work, so
//! a `#[scheduled]`/`#[processor]` reaches `Repo`, and settles what the job
//! wrote.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_database::{
    ExecutorScope, Settlement, TransactionPolicy, UnitOfWork, UnitSpec, run_unit,
};
use nest_rs_seaorm::{Executor, SeaOrmConfig, SeaOrmUnitOfWork, current_executor};
use sea_orm::{ConnectionTrait, Statement};

/// A table recreated per test, so a rollback is observable as a row count.
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

/// A queue attempt's unit of work, as the queue edge declares it.
const JOB: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
    "nest-rs-queue",
    "nest_rs::queue",
    nest_rs_core::operation_log::Kind::Consumer,
    "queue.job",
);

/// The binding, run the way the queue edge runs an attempt: the job's `bool`
/// is its outcome, and the answer is how its unit settled.
struct Attempts(Arc<dyn UnitOfWork>);

impl Attempts {
    async fn scope(
        &self,
        policy: TransactionPolicy,
        job: Pin<Box<dyn Future<Output = bool> + Send + '_>>,
    ) -> Settlement {
        let spec = UnitSpec::new(JOB, ExecutorScope::Job, policy);
        let ran = run_unit(
            Some(&self.0),
            spec,
            async move { if job.await { Ok(()) } else { Err(None) } },
            Result::is_ok,
            |why| Err(Some(why)),
        )
        .await;
        match ran {
            Err(Some(why)) => Settlement::Unhonoured(why),
            Ok(()) | Err(None) => Settlement::Settled,
        }
    }
}

fn context(conn: Arc<sea_orm::DatabaseConnection>) -> Attempts {
    Attempts(Arc::new(SeaOrmUnitOfWork::new(
        conn,
        &SeaOrmConfig::default(),
    )))
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
        ctx.scope(TransactionPolicy::PerUnit, job).await,
        Settlement::Settled
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

    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async {
        current_executor()
            .expect("ambient executor")
            .execute_unprepared("INSERT INTO worker_rollback_probe DEFAULT VALUES")
            .await
            .expect("the insert runs");
        false
    });
    assert_eq!(
        ctx.scope(TransactionPolicy::PerUnit, job).await,
        Settlement::Settled
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
        ctx.scope(TransactionPolicy::Pool, job).await,
        Settlement::Settled
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

    // Postgres aborts the transaction on the first failed statement, then
    // succeeds the `COMMIT` while rolling back.
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

    let settlement = ctx.scope(TransactionPolicy::PerUnit, job).await;
    let Settlement::Unhonoured(why) = settlement else {
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

/// A constraint checked at `COMMIT`, refused identically on every attempt: the
/// two rows the job writes are what collide.
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

    let Settlement::Unhonoured(why) = ctx.scope(TransactionPolicy::PerUnit, job).await else {
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

/// A `40001` serialization conflict, which the next attempt clears; Postgres
/// may raise it at the write or at the `COMMIT`, classified the same.
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
        // Postgres accepts it only before a query has fixed the snapshot.
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

    let Settlement::Unhonoured(why) = ctx.scope(TransactionPolicy::PerUnit, job).await else {
        panic!("the losing transaction wrote nothing, so the attempt is a failure");
    };
    assert!(
        why.retryable,
        "a `40001` is what a retry budget is for — refusing to spend it here \
         would dead-letter work that the next attempt completes: {why}",
    );
}

/// A job whose `COMMIT` the database refuses, though its body succeeded.
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
        true
    });

    let settlement = ctx.scope(TransactionPolicy::PerUnit, job).await;
    let Settlement::Unhonoured(why) = settlement else {
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

    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "transaction commit failed");
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

/// Work held for the commit of a job attempt runs after it, on the pool, and
/// sees what the attempt committed.
#[tokio::test]
async fn work_held_for_the_commit_runs_once_the_attempt_committed() {
    use crate::harness::{Sighting, Sightings, fresh_table, sighted, write_and_hold};
    const TABLE: &str = "after_commit_worker_commit";
    let conn = crate::harness::connect_arc().await;
    fresh_table(&conn, TABLE).await;
    let sightings = Sightings::default();
    let ctx = context(conn.clone());

    let (observer, record) = (conn.clone(), sightings.clone());
    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async move {
        write_and_hold(TABLE, observer, record).await;
        true
    });
    assert_eq!(
        ctx.scope(TransactionPolicy::PerUnit, job).await,
        Settlement::Settled
    );
    assert_eq!(
        sighted(&sightings),
        [Sighting {
            committed: 1,
            on_pool: true
        }],
    );
}

/// A failed attempt rolls back, and what waited for its commit never runs — so
/// the retry that follows is the only one that announces anything.
#[tokio::test]
async fn work_held_by_an_attempt_that_failed_never_runs() {
    use crate::harness::{Sightings, committed_rows, fresh_table, sighted, write_and_hold};
    const TABLE: &str = "after_commit_worker_rollback";
    let conn = crate::harness::connect_arc().await;
    fresh_table(&conn, TABLE).await;
    let sightings = Sightings::default();
    let ctx = context(conn.clone());

    let (observer, record) = (conn.clone(), sightings.clone());
    let job: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async move {
        write_and_hold(TABLE, observer, record).await;
        false
    });
    assert_eq!(
        ctx.scope(TransactionPolicy::PerUnit, job).await,
        Settlement::Settled
    );
    assert_eq!(committed_rows(&conn, TABLE).await, 0);
    assert!(sighted(&sightings).is_empty());
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct DeadlineProbe {
    id: i32,
}

#[nest_rs_queue::queue(name = "worker-deadline", job = DeadlineProbe)]
struct DeadlineQueue;

/// How many times the listener on the job's committed write ran.
static LISTENER_RUNS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The test's word to that listener, given once the attempt answered.
static LISTENER_RELEASED: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct DeadlineProcessor;

impl nest_rs_core::ProviderResidency for DeadlineProcessor {
    const SINGLETON: bool = true;
}

#[nest_rs_queue::processor]
impl DeadlineProcessor {
    /// Writes and returns within its timeout; what listens to the write
    /// outlives it, waiting for the test's word.
    #[process(queue = DeadlineQueue, retries = 1, timeout = "1s")]
    async fn write(&self, _job: DeadlineProbe) -> anyhow::Result<()> {
        current_executor()
            .expect("the attempt runs with an ambient executor")
            .execute_unprepared("INSERT INTO worker_deadline_probe DEFAULT VALUES")
            .await?;
        nest_rs_database::after_commit(async {
            LISTENER_RELEASED.notified().await;
            LISTENER_RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
        Ok(())
    }
}

static BARE: nest_rs_queue::QueueBackend =
    nest_rs_queue::QueueBackend::new("bare", nest_rs_queue::Capabilities::NONE);

/// The timeout bounds the body: a job that answered in time is never retried
/// because what listens to its commit outlives it, so its write lands once and
/// its listener runs once.
#[tokio::test]
async fn a_job_whose_listener_outlives_its_timeout_is_not_retried() {
    use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};

    let conn = crate::harness::connect_arc().await;
    scratch_table(&conn, "worker_deadline_probe").await;
    let unit: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(
        conn.clone(),
        &SeaOrmConfig::default(),
    ));
    let method = nest_rs_core::inventory::iter::<nest_rs_queue::ProcessMethod>()
        .find(|method| method.name() == "DeadlineProcessor::write")
        .expect("the processor's method is submitted");
    let mut delivery = Delivery::new(
        &BARE,
        nest_rs_queue::QueueName::new("worker-deadline").expect("a valid name"),
        serde_json::json!({
            "v": nest_rs_queue::WIRE_FORMAT_VERSION,
            "payload": { "id": 1 },
        }),
    );

    let outcome = consume::attempt(
        method,
        &mut delivery,
        nest_rs_core::Container::builder()
            .provide(DeadlineProcessor)
            .build()
            .expect("the container builds"),
        Some(unit),
    )
    .await;
    assert!(
        matches!(outcome, AttemptOutcome::Ok),
        "a commit whose listener outlives the timeout is no timeout: {outcome:?}"
    );
    assert_eq!(row_count(&conn, "worker_deadline_probe").await, 1);
    LISTENER_RELEASED.notify_one();
    nest_rs_testing::wait_until(std::time::Duration::from_secs(5), || {
        LISTENER_RUNS.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        LISTENER_RUNS.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the listener ran once, on the driver's task"
    );
}

#[nest_rs_queue::queue(name = "worker-atomic-in-doubt", job = DeadlineProbe)]
struct AtomicInDoubtQueue;

struct AtomicInDoubtProcessor;

impl nest_rs_core::ProviderResidency for AtomicInDoubtProcessor {
    const SINGLETON: bool = true;
}

#[nest_rs_queue::processor]
impl AtomicInDoubtProcessor {
    /// On the pool, its one write in an atomic block of its own, whose
    /// `COMMIT` outlives the timeout.
    #[process(queue = AtomicInDoubtQueue, retries = 1, timeout = "1s", transactional = false)]
    async fn write(&self, _job: DeadlineProbe) -> anyhow::Result<()> {
        nest_rs_database::atomic(async {
            current_executor()
                .expect("the block runs with an ambient executor")
                .execute_unprepared("INSERT INTO worker_atomic_in_doubt DEFAULT VALUES")
                .await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
    }
}

/// A deadline cutting the body while its atomic block commits never cuts that
/// `COMMIT`: it lands on the driver's task, and the attempt is in doubt —
/// dead-lettered, never retried into a second write. The `COMMIT` waits for
/// the test's word, so an attempt that waited for it would answer only at the
/// settle net, sixteen seconds past the deadline.
#[tokio::test]
async fn a_job_cut_while_its_atomic_block_commits_is_in_doubt_and_its_write_lands() {
    use nest_rs_queue::__private::consume::{self, AttemptOutcome, Delivery};
    use sea_orm::TransactionTrait;

    let conn = crate::harness::connect_arc().await;
    scratch_table(&conn, "worker_atomic_in_doubt").await;
    let gate = crate::harness::advisory_lock_key("worker_atomic_in_doubt_commit");
    // A deferred trigger runs at `COMMIT`, which then waits for the gate.
    for sql in [
        format!(
            "CREATE OR REPLACE FUNCTION worker_atomic_in_doubt_slow() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({gate}); \
             RETURN NULL; END $$"
        ),
        "CREATE CONSTRAINT TRIGGER worker_atomic_in_doubt_slow AFTER INSERT ON \
         worker_atomic_in_doubt DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION \
         worker_atomic_in_doubt_slow()"
            .to_owned(),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .expect("the gated commit is set up");
    }
    let word = conn.begin().await.expect("the gate's transaction opens");
    word.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({gate})"))
        .await
        .expect("the gate is held");
    let unit: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(
        conn.clone(),
        &SeaOrmConfig::default(),
    ));
    let method = nest_rs_core::inventory::iter::<nest_rs_queue::ProcessMethod>()
        .find(|method| method.name() == "AtomicInDoubtProcessor::write")
        .expect("the processor's method is submitted");
    let mut delivery = Delivery::new(
        &BARE,
        nest_rs_queue::QueueName::new("worker-atomic-in-doubt").expect("a valid name"),
        serde_json::json!({
            "v": nest_rs_queue::WIRE_FORMAT_VERSION,
            "payload": { "id": 1 },
        }),
    );

    let started = std::time::Instant::now();
    let outcome = consume::attempt(
        method,
        &mut delivery,
        nest_rs_core::Container::builder()
            .provide(AtomicInDoubtProcessor)
            .build()
            .expect("the container builds"),
        Some(unit),
    )
    .await;
    let took = started.elapsed();
    let AttemptOutcome::DeadLetter(error) = outcome else {
        panic!("a commit the cut left running may have landed: {outcome:?}");
    };
    assert!(
        error.to_string().ends_with("its writes may have landed"),
        "{error}"
    );
    assert!(
        took < std::time::Duration::from_secs(10),
        "answered at the deadline, never after the commit the gate holds, took {took:?}",
    );
    word.rollback()
        .await
        .expect("the gate lets the commit through");
    nest_rs_testing::wait_for(std::time::Duration::from_secs(5), || {
        let conn = Arc::clone(&conn);
        async move { row_count(&conn, "worker_atomic_in_doubt").await == 1 }
    })
    .await;
}
