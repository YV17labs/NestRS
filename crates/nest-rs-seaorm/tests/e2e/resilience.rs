//! A resilience policy's attempts over SeaORM: a call declared idempotent runs
//! each attempt in a savepoint of its unit's transaction, so a failed attempt
//! leaves no write for the next to repeat — rolled back once it failed, was cut
//! at its timeout mid-statement or met a statement the database refused, what
//! its unit wrote before it standing — whatever retries its policy has, none
//! included; a method declaring a fallback runs its attempts so too, the
//! failed one rolled back before the fallback writes on the unit's own
//! transaction, or its filter hands the error back; an attempt cut before its
//! savepoint answered leaves its unit as it was; two calls joined in one unit
//! take its turn one after the other; and nothing is sent for a call that
//! never queries, nor on the pool, which holds no transaction.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use nest_rs_core::{injectable, instance, module};
use nest_rs_database::__private::with_request_executor;
use nest_rs_resilience::{
    BreakerState, ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
};
use nest_rs_seaorm::__private::{
    FinalizeOutcome, LazyTransaction, SeaOrmExecutor, current_executor,
};
use nest_rs_testing::HeadlessApp;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

#[instance]
struct Ledger;

/// A dependency whose policy has its retries off.
#[instance]
struct Archive;

/// A dependency whose breaker opens on one failure.
#[instance]
struct Tracker;

#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("the ledger's statement failed")]
    Db(#[from] DbErr),
    #[error("the ledger refused the entry")]
    Refused,
}

/// A service writing its ledger under the policy, as an app's does: through
/// the ambient executor, each attempt of a method declared idempotent.
#[injectable]
struct LedgerService {
    #[inject]
    ledger: Arc<ResiliencePolicy<Ledger>>,
    #[inject]
    archive: Arc<ResiliencePolicy<Archive>>,
    #[inject]
    tracker: Arc<ResiliencePolicy<Tracker>>,
    attempts: AtomicU32,
}

impl LedgerService {
    /// One row of `entry`, the first attempt refused once it wrote its own.
    #[resilient(ledger, idempotent)]
    async fn post(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        write(entry).await?;
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(Failure::Refused);
        }
        Ok(())
    }

    /// One row of `entry`, the first attempt cut at its timeout while a
    /// statement it sent after writing is still running.
    #[resilient(ledger, idempotent, timeout = "200ms")]
    async fn post_slowly(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        write(entry).await?;
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            outlast_the_timeout().await?;
        }
        Ok(())
    }

    /// One row of `entry`, each attempt given up at 200 ms, whatever its
    /// first statement waits on.
    #[resilient(ledger, idempotent, timeout = "200ms")]
    async fn post_quickly(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        write(entry).await?;
        Ok(())
    }

    /// One row of `entry`, then a call to a dependency answering in
    /// [`ANSWERS_IN`], inside the 300 ms its one attempt has.
    #[resilient(tracker, idempotent, timeout = "300ms")]
    async fn post_then_track(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        write(entry).await?;
        tokio::time::sleep(ANSWERS_IN).await;
        Ok(())
    }

    /// One row of `entry`, every attempt refused once it wrote its own.
    #[resilient(archive, idempotent)]
    async fn archive(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        write(entry).await?;
        Err(Failure::Refused)
    }

    /// One row of `entry`, sent once and refused once it wrote its own: the
    /// fallback answers in its place when `falls_back`.
    #[resilient(archive, fallback = write_instead, fallback_if = |_| falls_back)]
    async fn archive_or_fall_back(
        &self,
        entry: Uuid,
        fallen: Uuid,
        falls_back: bool,
    ) -> Result<(), ResilienceError<Failure>> {
        write(entry).await?;
        Err(Failure::Refused)
    }

    /// One row of `fallen`, in place of the entry the archive refused.
    async fn write_instead(
        &self,
        _entry: Uuid,
        fallen: Uuid,
        _falls_back: bool,
        _error: ResilienceError<Failure>,
    ) -> Result<(), ResilienceError<Failure>> {
        write(fallen)
            .await
            .map_err(|refused| ResilienceError::Failed(refused.into()))
    }
}

#[module(
    imports = [
        ResilienceModule::for_root((Ledger, ResilienceConfig {
            retries: Some(2),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: None,
            ..ResilienceConfig::default()
        })),
        ResilienceModule::for_root((Archive, ResilienceConfig {
            retries: None,
            breaker_window: None,
            ..ResilienceConfig::default()
        })),
        ResilienceModule::for_root((Tracker, ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        })),
    ],
    providers = [LedgerService],
)]
struct LedgerModule;

/// How long the tracker takes to answer [`LedgerService::post_then_track`]:
/// two thirds of that attempt's bound.
const ANSWERS_IN: Duration = Duration::from_millis(200);

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "resilience_probe",
        "CREATE TABLE IF NOT EXISTS resilience_probe (id UUID PRIMARY KEY, entry UUID NOT NULL);",
    )
    .await;
    conn
}

async fn booted() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<LedgerModule>()
        .build_headless()
        .await
        .expect("the policy boots")
}

async fn policy() -> (HeadlessApp, Arc<ResiliencePolicy<Ledger>>) {
    let app = booted().await;
    let policy = app
        .container()
        .get::<ResiliencePolicy<Ledger>>()
        .expect("the module built the policy");
    (app, policy)
}

async fn service() -> (HeadlessApp, Arc<LedgerService>) {
    let app = booted().await;
    let service = app
        .container()
        .get::<LedgerService>()
        .expect("the module lists the service");
    (app, service)
}

/// One row of `entry`, written on the ambient executor.
async fn write(entry: Uuid) -> Result<(), DbErr> {
    write_as(Uuid::now_v7(), entry).await
}

/// The row `id` of `entry`, written on the ambient executor.
async fn write_as(id: Uuid, entry: Uuid) -> Result<(), DbErr> {
    let executor = current_executor().ok_or_else(|| DbErr::Custom("no executor".into()))?;
    executor
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO resilience_probe (id, entry) VALUES ($1, $2)",
            [id.into(), entry.into()],
        ))
        .await
        .map(|_| ())
}

/// A statement on the ambient executor still running at the 200 ms an attempt
/// of [`LedgerService::post_slowly`] is given up at: the test lasts as long,
/// since the rollback waits for it.
async fn outlast_the_timeout() -> Result<(), DbErr> {
    let executor = current_executor().ok_or_else(|| DbErr::Custom("no executor".into()))?;
    executor
        .execute_unprepared("SELECT pg_sleep(1)")
        .await
        .map(|_| ())
}

fn unit(conn: &DatabaseConnection) -> Arc<LazyTransaction> {
    Arc::new(LazyTransaction::new(conn.clone(), "test"))
}

async fn committed(unit: Arc<LazyTransaction>) {
    let outcome = unit.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Committed),
        "the unit commits what it wrote and what its run kept, got {outcome:?}"
    );
}

/// How many rows of `entry` the table holds.
async fn written(conn: &DatabaseConnection, entry: Uuid) -> i64 {
    conn.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COUNT(*) AS rows FROM resilience_probe WHERE entry = $1",
        [entry.into()],
    ))
    .await
    .expect("the probe reads")
    .expect("a count answers one row")
    .try_get("", "rows")
    .expect("the count")
}

#[tokio::test]
async fn an_attempt_that_failed_leaves_no_write_for_the_next_to_repeat() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let entry = Uuid::now_v7();
    let attempts = AtomicU32::new(0);

    let lazy = Arc::new(LazyTransaction::new(conn.clone(), "test"));
    let ran = with_request_executor(
        SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
        policy.run_idempotent(|_| async {
            write(entry).await?;
            if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(Failure::Refused);
            }
            Ok(())
        }),
    )
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Committed),
        "the unit commits what its last attempt wrote, got {outcome:?}"
    );

    assert_eq!(
        written(&conn, entry).await,
        1,
        "the failed attempt's row was rolled back before the next attempt wrote its own"
    );
}

/// A decorated method's attempts each run in a savepoint: the refused one is
/// rolled back alone, and what its unit wrote before the call stands.
#[tokio::test]
async fn a_decorated_methods_failed_attempt_is_rolled_back_alone_and_its_units_earlier_write_stands()
 {
    let conn = db().await;
    let (_app, service) = service().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        service.post(entry).await
    })
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(service.attempts.load(Ordering::Relaxed), 2);
    committed(lazy).await;

    assert_eq!(written(&conn, before).await, 1);
    assert_eq!(written(&conn, entry).await, 1);
}

/// An attempt given up at its timeout leaves a statement running: its future
/// is dropped there, and its rollback is awaited behind that statement, so the
/// unit stays able to commit what the retry writes.
#[tokio::test]
async fn an_attempt_cut_at_its_timeout_mid_statement_is_rolled_back_and_its_retry_commits() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = db().await;
    let (_app, service) = service().await;
    let entry = Uuid::now_v7();

    let lazy = unit(&conn);
    let ran = with_request_executor(
        SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
        service.post_slowly(entry),
    )
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(service.attempts.load(Ordering::Relaxed), 2);
    committed(lazy).await;

    assert_eq!(
        written(&conn, entry).await,
        1,
        "the row the cut attempt wrote was rolled back, the retry's committed"
    );
    logs.expect_none(
        nest_rs_seaorm::target::ORM,
        "atomic block abandoned before it settled; its unit cannot commit",
    );
}

/// A statement Postgres refuses aborts its transaction up to the last
/// savepoint: the attempt's, so the retry writes on and the unit commits.
#[tokio::test]
async fn a_statement_refused_inside_an_attempt_leaves_its_retry_free_to_write_and_commit() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let (taken, entry) = (Uuid::now_v7(), Uuid::now_v7());
    let attempts = AtomicU32::new(0);

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write_as(taken, entry).await.expect("the unit writes first");
        policy
            .run_idempotent(|_| async {
                let id = match attempts.fetch_add(1, Ordering::Relaxed) {
                    0 => taken,
                    _ => Uuid::now_v7(),
                };
                Ok::<_, Failure>(write_as(id, entry).await?)
            })
            .await
    })
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    committed(lazy).await;

    assert_eq!(
        written(&conn, entry).await,
        2,
        "the unit's row and the retry's"
    );
}

/// A run ending on its last attempt's error leaves its unit as it found it:
/// able to commit, what it wrote before the run standing, nothing of an attempt.
#[tokio::test]
async fn a_run_whose_last_attempt_fails_leaves_its_unit_its_earlier_write_and_nothing_of_an_attempt()
 {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());
    let attempts = AtomicU32::new(0);

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        policy
            .run_idempotent(|_| async {
                attempts.fetch_add(1, Ordering::Relaxed);
                write(entry).await?;
                Err::<(), _>(Failure::Refused)
            })
            .await
    })
    .await;
    assert!(
        matches!(ran, Err(ResilienceError::Failed(Failure::Refused))),
        "{ran:?}"
    );
    assert_eq!(attempts.load(Ordering::Relaxed), 3);
    committed(lazy).await;

    assert_eq!(written(&conn, before).await, 1);
    assert_eq!(
        written(&conn, entry).await,
        0,
        "the last attempt is rolled back as the others are"
    );
}

/// What a failed call leaves is read off its declaration, never off the
/// retries its policy has: under a policy with none, an idempotent method is
/// attempted once and that attempt rolled back, so its unit commits what it
/// wrote before the call and nothing of it.
#[tokio::test]
async fn an_idempotent_call_under_a_policy_with_no_retries_leaves_no_write_once_it_failed() {
    let conn = db().await;
    let (_app, service) = service().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        service.archive(entry).await
    })
    .await;
    assert!(
        matches!(ran, Err(ResilienceError::Failed(Failure::Refused))),
        "{ran:?}"
    );
    assert_eq!(
        service.attempts.load(Ordering::Relaxed),
        1,
        "not attempted again"
    );
    committed(lazy).await;

    assert_eq!(written(&conn, before).await, 1);
    assert_eq!(
        written(&conn, entry).await,
        0,
        "the one attempt is rolled back, as the last is with retries on"
    );
}

/// A savepoint is sent with its attempt's first statement: a call that never
/// queries, attempted twice, opens no transaction at all.
#[tokio::test]
async fn a_call_that_never_queries_sends_nothing() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let attempts = AtomicU32::new(0);

    let lazy = unit(&conn);
    let ran = with_request_executor(
        SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
        policy.run_idempotent(|_| async {
            if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(Failure::Refused);
            }
            Ok(())
        }),
    )
    .await;
    assert!(ran.is_ok(), "{ran:?}");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);

    assert!(!lazy.is_opened(), "no BEGIN, no SAVEPOINT");
    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::NoTransaction),
        "{outcome:?}"
    );
}

/// Nothing can be awaited as a run is dropped: its attempt's savepoint goes as
/// the port says of a block dropped before it settled, and its unit commits
/// nothing.
#[tokio::test]
async fn a_run_dropped_mid_attempt_leaves_its_unit_unable_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = db().await;
    let (_app, policy) = policy().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());
    let wrote = tokio::sync::Notify::new();

    let lazy = unit(&conn);
    with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        let run = policy.run_idempotent(|_| async {
            write(entry).await?;
            wrote.notify_one();
            std::future::pending::<Result<(), Failure>>().await
        });
        tokio::select! {
            ran = run => panic!("the attempt never answers: {ran:?}"),
            () = wrote.notified() => {}
        }
    })
    .await;

    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Poisoned { retryable: false }),
        "{outcome:?}"
    );
    assert_eq!(written(&conn, before).await, 0);
    assert_eq!(written(&conn, entry).await, 0);
    let line = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "atomic block abandoned before it settled; its unit cannot commit",
    );
    assert_eq!(line.level, "warn");
}

/// Under a fallback, a failed attempt leaves no write: its savepoint is
/// rolled back before the fallback runs, whose own write joins the unit's
/// transaction with what the unit wrote before the call, and before the
/// method answers an error its filter refused, the unit still free to commit.
#[tokio::test]
async fn under_a_fallback_a_failed_attempts_write_is_rolled_back_and_the_fallbacks_commits_with_its_unit()
 {
    let conn = db().await;
    let (_app, service) = service().await;

    let (before, entry, fallen) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let lazy = unit(&conn);
    let answered = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        service.archive_or_fall_back(entry, fallen, true).await
    })
    .await;
    assert!(answered.is_ok(), "the fallback answers: {answered:?}");
    committed(lazy).await;
    assert_eq!(
        (
            written(&conn, before).await,
            written(&conn, entry).await,
            written(&conn, fallen).await,
        ),
        (1, 0, 1),
        "the unit's earlier write and the fallback's stand, the failed attempt's does not"
    );

    let (before, entry, fallen) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let lazy = unit(&conn);
    let refused = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        service.archive_or_fall_back(entry, fallen, false).await
    })
    .await;
    assert!(
        matches!(refused, Err(ResilienceError::Failed(Failure::Refused))),
        "the error its filter refused is the method's: {refused:?}"
    );
    committed(lazy).await;
    assert_eq!(
        (
            written(&conn, before).await,
            written(&conn, entry).await,
            written(&conn, fallen).await,
        ),
        (1, 0, 0),
        "the unit's earlier write stands alone"
    );
}

/// The pool holds no transaction to open a savepoint in, and an attempt opens
/// none of its own: each statement commits as it is sent, a failed attempt's
/// included, as with no policy.
#[tokio::test]
async fn on_the_pool_an_attempt_opens_no_transaction_and_its_writes_commit_as_they_are_sent() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let entry = Uuid::now_v7();
    let attempts = AtomicU32::new(0);

    let ran = with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        policy.run_idempotent(|_| async {
            let on_the_pool = matches!(current_executor(), Some(SeaOrmExecutor::Pool(_)));
            write(entry).await?;
            if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(Failure::Refused);
            }
            Ok(on_the_pool)
        }),
    )
    .await;
    assert!(matches!(ran, Ok(true)), "{ran:?}");

    assert_eq!(
        written(&conn, entry).await,
        2,
        "a write on the pool is its statement's own commit: no attempt undoes it"
    );
}

const ABANDONED: &str = "atomic block abandoned before it settled; its unit cannot commit";

/// Once `service` made its attempt number `number`: the one before it was
/// given up at its timeout, whatever it waited on still withheld.
async fn attempted(service: &LedgerService, number: u32) {
    nest_rs_testing::wait_until(Duration::from_secs(10), || {
        service.attempts.load(Ordering::Relaxed) >= number
    })
    .await;
}

/// An attempt cut before its `SAVEPOINT` answered ran nothing of its own in
/// its unit, which stays as it was. First of the three waits its first
/// statement can be cut in: the pool's one connection is lent elsewhere, so the
/// unit's transaction is not begun, and nothing was sent.
#[tokio::test]
async fn an_attempt_cut_while_its_unit_waits_for_a_connection_leaves_its_unit_able_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let _table = db().await;
    let mut options = crate::harness::options(crate::harness::url());
    options.max_connections(1).min_connections(0);
    let one = sea_orm::Database::connect(options)
        .await
        .expect("a pool of one connection");
    let (_app, service) = service().await;
    let entry = Uuid::now_v7();

    let lent = sea_orm::TransactionTrait::begin(&one)
        .await
        .expect("the pool's one connection");
    let lazy = unit(&one);
    let (ran, ()) = tokio::join!(
        with_request_executor(
            SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
            service.post_quickly(entry),
        ),
        async {
            attempted(&service, 2).await;
            lent.rollback()
                .await
                .expect("the connection is handed back");
        },
    );
    assert!(
        ran.is_ok(),
        "an attempt answers once a connection is free: {ran:?}"
    );
    logs.expect_none(nest_rs_seaorm::target::ORM, ABANDONED);
    committed(lazy).await;

    assert_eq!(written(&one, entry).await, 1);
}

/// The `SSLRequest` a Postgres client opens with, answered `S`.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

/// A double between a pool and the suite's Postgres, ending TLS on both
/// sides: it passes on every byte as it comes, but what Postgres answers
/// while it is told to hold, so a statement sent stays unanswered for as long
/// as a test keeps it there — a `BEGIN`'s wait or a `SAVEPOINT`'s, which no
/// `pg_sleep` reaches.
struct AnswersHeld {
    url: String,
    _authority: tempfile::NamedTempFile,
    holding: tokio::sync::watch::Sender<bool>,
    /// What the pool sent while its answers were held.
    sent: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl AnswersHeld {
    async fn start() -> Self {
        use std::str::FromStr as _;

        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let authority = nest_rs_testing::TestAuthority::new();
        let root = tempfile::NamedTempFile::new().expect("a file for the authority");
        std::fs::write(root.path(), authority.pem()).expect("the authority's certificate");
        let acceptor = authority.server(&["127.0.0.1", "localhost"]).acceptor(None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("its address");

        let suite = crate::harness::url();
        let postgres =
            sqlx::postgres::PgConnectOptions::from_str(&suite).expect("the suite's Postgres URL");
        let (host, port) = (postgres.get_host().to_owned(), postgres.get_port());
        let (holding, _) = tokio::sync::watch::channel(false);
        let sent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let relayed = (holding.clone(), Arc::clone(&sent));
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let (acceptor, host) = (acceptor.clone(), host.clone());
                let (holding, sent) = relayed.clone();
                tokio::spawn(async move {
                    let mut request = [0_u8; 8];
                    if client.read_exact(&mut request).await.is_err()
                        || request != SSL_REQUEST
                        || client.write_all(b"S").await.is_err()
                    {
                        return;
                    }
                    let Ok(client) = acceptor.accept(client).await else {
                        return;
                    };
                    let Ok(mut server) =
                        tokio::net::TcpStream::connect((host.as_str(), port)).await
                    else {
                        return;
                    };
                    let mut encrypts = [0_u8; 1];
                    if server.write_all(&SSL_REQUEST).await.is_err()
                        || server.read_exact(&mut encrypts).await.is_err()
                        || encrypts != *b"S"
                    {
                        return;
                    }
                    let Ok(name) = host.try_into() else {
                        return;
                    };
                    let Ok(server) = nest_rs_testing::system_connector()
                        .connect(name, server)
                        .await
                    else {
                        return;
                    };
                    let (mut asked, mut answering) = tokio::io::split(client);
                    let (mut answered, mut asking) = tokio::io::split(server);
                    let asks = async {
                        let mut bytes = [0_u8; 8192];
                        while let Ok(read @ 1..) = asked.read(&mut bytes).await {
                            if *holding.borrow() {
                                sent.lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .extend_from_slice(&bytes[..read]);
                            }
                            if asking.write_all(&bytes[..read]).await.is_err()
                                || asking.flush().await.is_err()
                            {
                                return;
                            }
                        }
                    };
                    let answers = async {
                        let mut released = holding.subscribe();
                        let mut bytes = [0_u8; 8192];
                        while let Ok(read @ 1..) = answered.read(&mut bytes).await {
                            if released.wait_for(|holding| !holding).await.is_err()
                                || answering.write_all(&bytes[..read]).await.is_err()
                                || answering.flush().await.is_err()
                            {
                                return;
                            }
                        }
                    };
                    tokio::select! {
                        () = asks => {}
                        () = answers => {}
                    }
                });
            }
        });
        let separator = if suite.contains('?') { '&' } else { '?' };
        Self {
            url: format!(
                "{}{separator}sslmode=verify-full&sslrootcert={}",
                nest_rs_testing::url_at(&suite, address),
                root.path().display()
            ),
            _authority: root,
            holding,
            sent,
        }
    }

    /// A pool through the double, lending a connection without asking Postgres
    /// whether it still answers: a `BEGIN` is then the first thing a unit
    /// sends.
    async fn pool(&self) -> DatabaseConnection {
        let mut options = crate::harness::options(self.url.clone());
        options.max_connections(1).test_before_acquire(false);
        let pool = sea_orm::Database::connect(options)
            .await
            .expect("the pool opens through the double");
        pool.execute_unprepared("SELECT 1")
            .await
            .expect("Postgres answers through the double");
        // A connection handed back is asked whether it answers before it is idle.
        nest_rs_testing::wait_until(Duration::from_secs(10), || {
            pool.get_postgres_connection_pool().num_idle() == 1
        })
        .await;
        pool
    }

    /// Hold back what Postgres answers from here on.
    fn hold(&self) {
        self.holding.send_replace(true);
    }

    /// Let through what was held, answering what the pool sent meanwhile.
    fn release(&self) -> Vec<u8> {
        self.holding.send_replace(false);
        std::mem::take(
            &mut self
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

fn holds(sent: &[u8], statement: &str) -> bool {
    sent.windows(statement.len())
        .any(|window| window == statement.as_bytes())
}

/// The second wait: the unit's `BEGIN` is sent and not answered. The pool
/// rolls back a transaction whose `BEGIN` was dropped unanswered before it
/// lends the connection again, and the unit, which never held it, begins
/// another.
#[tokio::test]
async fn an_attempt_cut_while_its_units_begin_is_unanswered_leaves_its_unit_able_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let _table = db().await;
    let double = AnswersHeld::start().await;
    let through = double.pool().await;
    let (_app, service) = service().await;
    let entry = Uuid::now_v7();

    let lazy = unit(&through);
    double.hold();
    let (ran, sent) = tokio::join!(
        with_request_executor(
            SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
            service.post_quickly(entry),
        ),
        async {
            attempted(&service, 2).await;
            double.release()
        },
    );
    assert!(
        holds(&sent, "BEGIN"),
        "the first attempt was cut with its unit's BEGIN sent"
    );
    assert!(
        ran.is_ok(),
        "an attempt answers once Postgres does: {ran:?}"
    );
    logs.expect_none(nest_rs_seaorm::target::ORM, ABANDONED);
    committed(lazy).await;

    assert_eq!(written(&through, entry).await, 1);
}

/// The third wait: the attempt's own `SAVEPOINT` is sent and not answered.
/// Postgres opens it all the same, and it holds nothing: no statement of the
/// attempt ran inside. The retry's `SAVEPOINT`, of the same name, hides it
/// until the unit commits, so the unit keeps what it wrote before the call
/// and what the retry writes.
#[tokio::test]
async fn an_attempt_cut_while_its_savepoint_is_unanswered_leaves_its_unit_able_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let _table = db().await;
    let double = AnswersHeld::start().await;
    let through = double.pool().await;
    let (_app, service) = service().await;
    let (before, first, entry) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    let lazy = unit(&through);
    let (ran, left_open) =
        with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
            write(before)
                .await
                .expect("the unit writes before the call");
            // The connection knows the statement from here on: sent again, it
            // is run at once, where a first one is only parsed.
            service
                .post_quickly(first)
                .await
                .expect("a call answering at once");
            double.hold();
            let (ran, _sent) = tokio::join!(service.post_quickly(entry), async {
                attempted(&service, 3).await;
                double.release()
            });
            let unit = current_executor().expect("the unit's executor");
            let left_open = unit
                .execute_unprepared("RELEASE SAVEPOINT nestrs_atomic_1")
                .await;
            (ran, left_open)
        })
        .await;
    assert!(
        ran.is_ok(),
        "an attempt answers once Postgres does: {ran:?}"
    );
    assert!(
        left_open.is_ok(),
        "the cut attempt's SAVEPOINT ran on the server, and was still open: {left_open:?}"
    );
    logs.expect_none(nest_rs_seaorm::target::ORM, ABANDONED);
    committed(lazy).await;

    for written_once in [before, first, entry] {
        assert_eq!(written(&through, written_once).await, 1);
    }
}

/// Savepoints are a stack on one connection, so two calls joined in one unit
/// run their database parts one after the other: the second's first statement
/// waits, inside its attempt's bound, until the first's savepoint settled —
/// here for the whole of the first's call to its dependency. A limit, pinned
/// as it stands: the tracker answered each call it was sent well inside the
/// bound, yet the call that waited is given up at it, and counted against the
/// tracker, whose breaker opens.
#[tokio::test]
async fn two_calls_joined_in_one_unit_take_its_turn_in_order_and_the_one_waiting_is_charged_the_wait()
 {
    let conn = db().await;
    let (app, service) = service().await;
    let tracker = app
        .container()
        .get::<ResiliencePolicy<Tracker>>()
        .expect("the module built the policy");
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = unit(&conn);
    let (went, waited) =
        with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
            tokio::join!(
                service.post_then_track(first),
                service.post_then_track(second)
            )
        })
        .await;

    assert!(
        went.is_ok(),
        "the call holding the unit's turn answers: {went:?}"
    );
    assert!(
        matches!(waited, Err(ResilienceError::TimedOut { .. })),
        "the call waiting for it spends {ANSWERS_IN:?} of its bound there: {waited:?}"
    );
    assert!(
        matches!(tracker.breaker_state(), Some(BreakerState::Open { .. })),
        "counted as the tracker's failure: {:?}",
        tracker.breaker_state()
    );
    committed(lazy).await;
    assert_eq!(written(&conn, first).await, 1);
    assert_eq!(written(&conn, second).await, 0, "given up, and rolled back");
}
