//! `SeaOrmOutbox` against Postgres, each test on a database of its own: a push
//! joins the transaction its unit holds, so its record lands exactly when the
//! unit's writes do, and the relay files what committed — once, through a
//! claim no two relays share. The relay's own decisions are proved in process
//! (`nest-rs-queue`'s outbox suite); what Postgres decides is proved here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{Transport, injectable, module};
use nest_rs_database::{
    AtomicError, ExecutorScope, OutboxStore, TransactionPolicy, UnitOfWork, UnitSpec, atomic,
    run_unit,
};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_http::{Response, StatusCode};
use nest_rs_queue::{
    Capabilities, Capability, Envelope, JobId, JobProducer, JobProducerExt, OutboxConfig,
    OutboxRelay, PushOptions, PushReceipt, QueueBackend, QueueError, QueueName, TARGET,
    async_trait,
};
use nest_rs_seaorm::queue::OutboxMigration;
use nest_rs_seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::{EphemeralDatabase, LogCapture, TestApp, wait_for, wait_until};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use sea_orm_migration::{MigrationTrait, MigratorTrait};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::harness::answered;

/// How long a test waits for the relay or for Postgres.
const PATIENCE: Duration = Duration::from_secs(30);

/// The outbox's table and nothing else.
struct OutboxMigrator;

impl MigratorTrait for OutboxMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(OutboxMigration)]
    }
}

/// A database whose outbox table was never created.
struct NoMigrations;

impl MigratorTrait for NoMigrations {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Note {
    n: u32,
}

#[nest_rs_queue::queue(name = "outbox_e2e", job = Note)]
struct NoteQueue;

/// A backend filing delayed, keyed and cancellable jobs.
static FILING: QueueBackend = QueueBackend::new(
    "filing",
    Capabilities::NONE
        .with(Capability::DelayedPush)
        .with(Capability::UniquePush)
        .with(Capability::Cancellation),
);

/// A backend that keeps every job it files, as the contract asks: an envelope
/// whose job it holds is skipped, a key another job holds is refused.
#[derive(Default)]
struct Filing {
    /// Every id each call handed over.
    calls: Mutex<Vec<Vec<JobId>>>,
    /// Every job it holds: id and unique key.
    held: Mutex<Vec<(JobId, Option<String>)>>,
    refusing: AtomicBool,
    /// Whether a call files, then never answers.
    silent_after_filing: AtomicBool,
    filed: Notify,
}

impl Filing {
    fn held(&self) -> Vec<JobId> {
        self.held
            .lock()
            .expect("lock")
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn handed(&self) -> Vec<JobId> {
        self.calls.lock().expect("lock").concat()
    }
}

#[async_trait]
impl JobProducer for Filing {
    fn backend(&self) -> &'static QueueBackend {
        &FILING
    }

    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        _options: &PushOptions,
    ) -> Result<(), QueueError> {
        if self.refusing.load(Ordering::SeqCst) {
            return Err(QueueError::backend(std::io::Error::other(
                "connection refused",
            )));
        }
        self.calls.lock().expect("lock").push(
            envelopes
                .iter()
                .map(|envelope| envelope.id().clone())
                .collect(),
        );
        {
            let mut held = self.held.lock().expect("lock");
            for envelope in &envelopes {
                if held.iter().any(|(id, _)| id == envelope.id()) {
                    continue;
                }
                if let Some(key) = envelope.unique_key()
                    && let Some((holder, _)) = held
                        .iter()
                        .find(|(_, holding)| holding.as_deref() == Some(key))
                {
                    return Err(QueueError::UniqueKeyHeld {
                        queue: queue.clone(),
                        key: key.to_owned(),
                        holder: holder.clone(),
                    });
                }
                held.push((
                    envelope.id().clone(),
                    envelope.unique_key().map(str::to_owned),
                ));
            }
        }
        self.filed.notify_one();
        if self.silent_after_filing.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    async fn remove(&self, _: &QueueName, id: &JobId) -> Result<bool, QueueError> {
        let mut held = self.held.lock().expect("lock");
        let before = held.len();
        held.retain(|(holding, _)| holding != id);
        Ok(held.len() < before)
    }

    async fn remove_unique(&self, _: &QueueName, key: &str) -> Result<bool, QueueError> {
        let mut held = self.held.lock().expect("lock");
        let before = held.len();
        held.retain(|(_, holding)| holding.as_deref() != Some(key));
        Ok(held.len() < before)
    }
}

/// What a request to `/outbox` answers, built per request.
type Answered =
    std::pin::Pin<Box<dyn std::future::Future<Output = nest_rs_http::Result<Response>> + Send>>;

/// What the outbox route runs as its handler, seeded per test.
struct Pushing(Box<dyn Fn() -> Answered + Send + Sync>);

impl Pushing {
    /// Run `body` on `filing`, then answer `200 OK`.
    fn running<F, Fut>(filing: &Arc<Filing>, body: F) -> Self
    where
        F: Fn(Arc<Filing>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let filing = Arc::clone(filing);
        Self(Box::new(move || {
            let ran = body(Arc::clone(&filing));
            Box::pin(async move {
                ran.await;
                Ok(answered(StatusCode::OK))
            })
        }))
    }

    /// Push `jobs` notes onto `filing` — one push, or one push of many — then
    /// answer `status`, noting each receipt in `receipts`.
    fn then_answer(
        filing: &Arc<Filing>,
        receipts: &Arc<Mutex<Vec<PushReceipt>>>,
        jobs: u32,
        status: StatusCode,
    ) -> Self {
        let (filing, receipts) = (Arc::clone(filing), Arc::clone(receipts));
        Self(Box::new(move || {
            let (filing, receipts) = (Arc::clone(&filing), Arc::clone(&receipts));
            Box::pin(async move {
                let pushed = if jobs == 1 {
                    vec![
                        filing
                            .push(NoteQueue, Note { n: 1 }, None)
                            .await
                            .expect("pushed"),
                    ]
                } else {
                    filing
                        .push_many(NoteQueue, (0..jobs).map(|n| Note { n }), None)
                        .await
                        .expect("pushed")
                };
                receipts.lock().expect("lock").extend(pushed);
                Ok(answered(status))
            })
        }))
    }
}

#[nest_rs_http::controller(path = "/outbox")]
struct OutboxController {
    #[inject]
    pushing: Arc<Pushing>,
}

#[nest_rs_http::routes]
impl OutboxController {
    #[get("/")]
    async fn read(&self) -> nest_rs_http::Result<Response> {
        (self.pushing.0)().await
    }

    #[post("/")]
    async fn write(&self) -> nest_rs_http::Result<Response> {
        (self.pushing.0)().await
    }
}

/// Asked of the listener: push one note.
#[derive(Clone)]
struct NoteAsked;

#[injectable]
struct NotingListeners {
    #[inject]
    queue: Arc<dyn JobProducer>,
}

#[listeners]
impl NotingListeners {
    #[on_event]
    async fn pushes_a_note(&self, _: NoteAsked) -> anyhow::Result<()> {
        self.queue.push(NoteQueue, Note { n: 7 }, None).await?;
        Ok(())
    }
}

#[module(
    imports = [SeaOrmModule::for_root(None), SeaOrmDatabaseModule, EventsModule],
    providers = [OutboxController, NotingListeners],
)]
struct OutboxAppModule;

/// An app over `db`, pushing onto `filing`, its route running `pushing`.
async fn app_over(db: &EphemeralDatabase, filing: &Arc<Filing>, pushing: Pushing) -> TestApp {
    TestApp::builder()
        .provide_arc(db.connection())
        .provide_dyn::<dyn JobProducer>(Arc::clone(filing) as Arc<dyn JobProducer>)
        .provide(pushing)
        .provide(OutboxConfig {
            sweep_interval: Duration::from_millis(100),
            stall_warning: Duration::from_secs(1),
            ..OutboxConfig::default()
        })
        .module::<OutboxAppModule>()
        .build()
        .await
        .expect("the app boots over its own database")
}

async fn database() -> EphemeralDatabase {
    EphemeralDatabase::create::<OutboxMigrator>()
        .await
        .expect("a database of its own, with the outbox's table")
}

/// Arm the app's store as its relay does at boot, without filing anything.
async fn arm(app: &TestApp) {
    app.container()
        .get_dyn::<dyn OutboxStore>()
        .expect("the binding keeps an outbox")
        .arm()
        .await
        .expect("the table is there");
}

/// The jobs the store keeps, oldest first.
async fn kept(db: &DatabaseConnection) -> Vec<String> {
    db.query_all_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT job_id::text AS id FROM nestrs_outbox ORDER BY seq",
    ))
    .await
    .expect("the store is read")
    .iter()
    .map(|row| row.try_get::<String>("", "id").expect("an id"))
    .collect()
}

fn ids(receipts: &Mutex<Vec<PushReceipt>>) -> Vec<String> {
    receipts
        .lock()
        .expect("lock")
        .iter()
        .map(|receipt| receipt.id().to_string())
        .collect()
}

/// A relay of `app`'s, serving until `stop` fires.
struct Relay {
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Relay {
    async fn of(app: &TestApp) -> Self {
        let mut relay = OutboxRelay::new();
        relay
            .configure(app.container())
            .await
            .expect("the relay arms its store");
        let stop = CancellationToken::new();
        let task = tokio::spawn(Box::new(relay).serve(stop.clone()));
        Self { stop, task }
    }

    async fn shutdown(self) {
        self.stop.cancel();
        self.task
            .await
            .expect("the relay's task ends")
            .expect("the relay stops cleanly");
    }
}

/// A unit of `app`'s binding running `body`, as a queue attempt does.
async fn in_a_unit<F>(app: &TestApp, body: F) -> Result<(), ()>
where
    F: std::future::Future<Output = Result<(), ()>> + Send,
{
    let binding = app
        .container()
        .get_dyn::<dyn UnitOfWork>()
        .expect("the binding");
    let unit = nest_rs_core::__private::declare_unit(
        "nest-rs-queue",
        "nest_rs::queue",
        nest_rs_core::operation_log::Kind::Consumer,
        "queue.job",
    );
    run_unit(
        Some(&binding),
        UnitSpec::new(unit, ExecutorScope::Job, TransactionPolicy::PerUnit),
        body,
        Result::is_ok,
        |_| Err(()),
    )
    .await
}

#[tokio::test]
async fn a_push_inside_a_request_answering_4xx_keeps_nothing_and_one_answering_2xx_is_filed_after_its_commit()
 {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let refused = Arc::new(Mutex::new(Vec::new()));
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &refused, 1, StatusCode::BAD_REQUEST),
    )
    .await;
    arm(&app).await;
    let answer = app.http().post("/outbox").send().await;
    assert_eq!(answer.0.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        refused.lock().expect("lock").len(),
        1,
        "the push itself answered"
    );
    assert!(
        kept(&db.connection()).await.is_empty(),
        "rolled back with the request"
    );

    // A second app on the same database: a closed app closes the pool it was seeded.
    let created = Arc::new(Mutex::new(Vec::new()));
    let creating = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &created, 1, StatusCode::CREATED),
    )
    .await;
    arm(&creating).await;
    let answer = creating.http().post("/outbox").send().await;
    assert_eq!(answer.0.status(), StatusCode::CREATED);
    assert_eq!(
        kept(&db.connection()).await,
        ids(&created),
        "committed with the request, under the receipt's id"
    );
    assert!(
        filing.handed().is_empty(),
        "nothing reached the backend inside the request"
    );

    let relay = Relay::of(&creating).await;
    wait_for(PATIENCE, || async {
        kept(&db.connection()).await.is_empty()
    })
    .await;
    assert_eq!(
        filing
            .held()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ids(&created),
        "filed once, after the commit"
    );
    relay.shutdown().await;
    creating.close().await;
    app.close().await;
}

#[tokio::test]
async fn a_push_of_many_inside_a_request_that_rolls_back_keeps_none() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let receipts = Arc::new(Mutex::new(Vec::new()));
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &receipts, 3, StatusCode::INTERNAL_SERVER_ERROR),
    )
    .await;
    arm(&app).await;
    app.http().post("/outbox").send().await;
    assert_eq!(
        receipts.lock().expect("lock").len(),
        3,
        "the push itself answered"
    );
    assert!(kept(&db.connection()).await.is_empty());
    assert!(filing.handed().is_empty());
    app.close().await;
}

#[tokio::test]
async fn a_push_on_a_safe_request_is_filed_at_once() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let receipts = Arc::new(Mutex::new(Vec::new()));
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &receipts, 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    app.http().get("/outbox").send().await;
    assert!(
        kept(&db.connection()).await.is_empty(),
        "a pool keeps no outbox"
    );
    assert_eq!(
        filing
            .handed()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ids(&receipts)
    );
    app.close().await;
}

#[tokio::test]
async fn a_push_from_a_listeners_unit_is_kept_by_its_transaction() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    let bus = app.container().get::<EventBus>().expect("the bus");
    in_a_unit(&app, async {
        bus.emit(NoteAsked).await;
        Ok(())
    })
    .await
    .expect("the emitter's unit commits");
    assert_eq!(
        kept(&db.connection()).await.len(),
        1,
        "the listener's own unit committed its record"
    );
    assert!(filing.handed().is_empty());
    app.close().await;
}

#[derive(Debug)]
struct Refused;

impl From<AtomicError> for Refused {
    fn from(_: AtomicError) -> Self {
        Self
    }
}

#[tokio::test]
async fn a_push_inside_an_atomic_block_that_rolls_back_is_dropped_while_its_unit_commits() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    let kept_id = Arc::new(Mutex::new(None));
    in_a_unit(&app, {
        let (filing, kept_id) = (Arc::clone(&filing), Arc::clone(&kept_id));
        async move {
            let outside = filing
                .push(NoteQueue, Note { n: 1 }, None)
                .await
                .expect("kept by the unit");
            *kept_id.lock().expect("lock") = Some(outside.id().to_string());
            let block = atomic(async {
                filing
                    .push(NoteQueue, Note { n: 2 }, None)
                    .await
                    .expect("kept by the block");
                Err::<(), _>(Refused)
            })
            .await;
            assert!(block.is_err(), "the block rolled back");
            Ok(())
        }
    })
    .await
    .expect("the unit commits");
    assert_eq!(
        kept(&db.connection()).await,
        [kept_id.lock().expect("lock").clone().expect("an id")],
        "the block's record went with its savepoint"
    );
    app.close().await;
}

#[tokio::test]
async fn a_cancel_inside_a_unit_deletes_the_record_no_relay_filed() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    let receipt = Arc::new(Mutex::new(None));
    in_a_unit(&app, {
        let (filing, receipt) = (Arc::clone(&filing), Arc::clone(&receipt));
        async move {
            *receipt.lock().expect("lock") = Some(
                filing
                    .push(NoteQueue, Note { n: 1 }, None)
                    .await
                    .expect("kept"),
            );
            Ok(())
        }
    })
    .await
    .expect("committed");
    assert_eq!(kept(&db.connection()).await.len(), 1);

    let receipt = receipt.lock().expect("lock").clone().expect("a receipt");
    in_a_unit(&app, {
        let filing = Arc::clone(&filing);
        async move {
            assert!(filing.cancel(&receipt).await.expect("cancelled"));
            Ok(())
        }
    })
    .await
    .expect("committed");
    assert!(kept(&db.connection()).await.is_empty());
    app.close().await;
}

/// How many statements on `db`'s database wait for a lock another holds.
async fn waiting_on_a_lock(db: &DatabaseConnection) -> i64 {
    db.query_one_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT count(*) AS waiting FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'",
    ))
    .await
    .expect("the server's activity is read")
    .expect("a row")
    .try_get("", "waiting")
    .expect("a count")
}

/// Cancel `receipt` inside a unit of `app`'s while the relay holds its record
/// claimed, the relay's side running `relay` once the cancel waits on its
/// lock: the cancel's answer.
async fn a_cancel_meeting_a_claim<R>(
    app: &TestApp,
    db: &DatabaseConnection,
    filing: &Arc<Filing>,
    receipt: PushReceipt,
    relay: R,
) -> bool
where
    R: std::future::Future<Output = ()>,
{
    let answer = Arc::new(Mutex::new(None));
    let cancelling = in_a_unit(app, {
        let (filing, answer) = (Arc::clone(filing), Arc::clone(&answer));
        async move {
            let cancelled = filing.cancel(&receipt).await.expect("the cancel answers");
            *answer.lock().expect("lock") = Some(cancelled);
            Ok(())
        }
    });
    let relaying = async {
        wait_for(PATIENCE, || async {
            let answered = answer.lock().expect("lock").is_some();
            answered || waiting_on_a_lock(db).await == 1
        })
        .await;
        assert_eq!(
            *answer.lock().expect("lock"),
            None,
            "the cancel answered while a relay held the record"
        );
        relay.await;
    };
    let (cancelled, ()) = tokio::join!(cancelling, relaying);
    cancelled.expect("the cancel's unit commits");
    let answered = answer.lock().expect("lock").take();
    answered.expect("the cancel answered")
}

#[tokio::test]
async fn a_cancel_meeting_a_record_a_relay_holds_waits_for_its_settle_and_never_answers_too_late() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    let store = app
        .container()
        .get_dyn::<dyn OutboxStore>()
        .expect("the binding keeps an outbox");

    let filed = committed_note(&app, &filing, None).await;
    let claim = store.claim(10).await.expect("claimed");
    let seq = claim.records()[0].seq;
    let cancelled = a_cancel_meeting_a_claim(&app, &db.connection(), &filing, filed.clone(), {
        let filing = Arc::clone(&filing);
        async move {
            filing
                .held
                .lock()
                .expect("lock")
                .push((filed.id().clone(), None));
            claim.settle(&[seq], &[]).await.expect("settled");
        }
    })
    .await;
    assert!(
        cancelled,
        "the relay filed it while the cancel waited: the backend cancels it"
    );
    assert!(filing.held().is_empty(), "the job never runs");

    let left = committed_note(&app, &filing, None).await;
    let claim = store.claim(10).await.expect("claimed");
    let cancelled = a_cancel_meeting_a_claim(&app, &db.connection(), &filing, left, async move {
        drop(claim);
    })
    .await;
    assert!(
        cancelled,
        "the relay let the record go unsettled: the cancel deletes it"
    );
    assert!(kept(&db.connection()).await.is_empty());
    assert!(
        filing.handed().is_empty(),
        "neither reached the backend twice"
    );
    app.close().await;
}

#[tokio::test]
async fn a_cancel_by_key_inside_a_unit_deletes_only_the_record_carrying_it() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    committed_note(&app, &filing, Some("song")).await;
    let other = committed_note(&app, &filing, Some("other")).await;

    in_a_unit(&app, {
        let filing = Arc::clone(&filing);
        async move {
            assert!(
                filing
                    .cancel_unique(NoteQueue, "song")
                    .await
                    .expect("cancelled")
            );
            Ok(())
        }
    })
    .await
    .expect("committed");
    assert_eq!(kept(&db.connection()).await, [other.id().to_string()]);
    app.close().await;
}

#[tokio::test]
async fn a_cancel_on_a_safe_request_deletes_the_committed_record_on_the_pool() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let receipt = Arc::new(Mutex::new(None::<PushReceipt>));
    let answer = Arc::new(Mutex::new(None));
    let pushing = Pushing::running(&filing, {
        let (receipt, answer) = (Arc::clone(&receipt), Arc::clone(&answer));
        move |filing| {
            let (receipt, answer) = (Arc::clone(&receipt), Arc::clone(&answer));
            async move {
                let receipt = receipt.lock().expect("lock").clone().expect("a receipt");
                let cancelled = filing.cancel(&receipt).await.expect("the cancel answers");
                *answer.lock().expect("lock") = Some(cancelled);
            }
        }
    });
    let app = app_over(&db, &filing, pushing).await;
    arm(&app).await;
    *receipt.lock().expect("lock") = Some(committed_note(&app, &filing, None).await);

    let answered = app.http().get("/outbox").send().await;
    assert_eq!(answered.0.status(), StatusCode::OK);
    assert_eq!(*answer.lock().expect("lock"), Some(true));
    assert!(kept(&db.connection()).await.is_empty());
    assert!(filing.handed().is_empty());
    app.close().await;
}

#[tokio::test]
async fn an_atomic_block_on_a_safe_request_keeps_its_push_until_it_commits_and_drops_it_on_rollback()
 {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let receipts = Arc::new(Mutex::new(Vec::new()));
    let pushing = Pushing::running(&filing, {
        let receipts = Arc::clone(&receipts);
        move |filing| {
            let receipts = Arc::clone(&receipts);
            async move {
                let committed = atomic(async {
                    let receipt = filing
                        .push(NoteQueue, Note { n: 1 }, None)
                        .await
                        .expect("kept by the block");
                    Ok::<_, Refused>(receipt)
                })
                .await
                .expect("the block commits");
                receipts.lock().expect("lock").push(committed);
                let rolled_back = atomic(async {
                    filing
                        .push(NoteQueue, Note { n: 2 }, None)
                        .await
                        .expect("kept by the block");
                    Err::<(), _>(Refused)
                })
                .await;
                assert!(rolled_back.is_err(), "the second block rolled back");
            }
        }
    });
    let app = app_over(&db, &filing, pushing).await;
    arm(&app).await;

    let answered = app.http().get("/outbox").send().await;
    assert_eq!(answered.0.status(), StatusCode::OK);
    assert_eq!(
        kept(&db.connection()).await,
        ids(&receipts),
        "the committed block's record, never the rolled-back one's"
    );
    assert!(
        filing.handed().is_empty(),
        "a push inside a block is not filed at once"
    );
    app.close().await;
}

/// Commit one note on `app`'s binding, keyed by `key` when given.
async fn committed_note(app: &TestApp, filing: &Arc<Filing>, key: Option<&str>) -> PushReceipt {
    let receipt = Arc::new(Mutex::new(None));
    in_a_unit(app, {
        let (filing, receipt) = (Arc::clone(filing), Arc::clone(&receipt));
        let options = key.map(|key| PushOptions::default().with_unique(key));
        async move {
            *receipt.lock().expect("lock") = Some(
                filing
                    .push(NoteQueue, Note { n: 1 }, options)
                    .await
                    .expect("kept"),
            );
            Ok(())
        }
    })
    .await
    .expect("committed");
    let committed = receipt.lock().expect("lock").clone();
    committed.expect("a receipt")
}

/// A relay killed after its call filed a note keyed by `key`, before its
/// settle, then another: the record is sent again, answered `Ok` and deleted,
/// and the job is filed once.
async fn a_relay_restarted_mid_pass(key: Option<&str>) {
    let logs = LogCapture::install();
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    let receipt = committed_note(&app, &filing, key).await;
    filing.silent_after_filing.store(true, Ordering::SeqCst);
    let filed = filing.filed.notified();
    let killed = Relay::of(&app).await;
    filed.await;
    killed.task.abort();
    assert!(killed.task.await.is_err_and(|ended| ended.is_cancelled()));

    filing.silent_after_filing.store(false, Ordering::SeqCst);
    let relay = Relay::of(&app).await;
    wait_for(PATIENCE, || async {
        kept(&db.connection()).await.is_empty()
    })
    .await;
    assert_eq!(filing.handed().len(), 2, "the next relay sent it again");
    assert_eq!(
        filing.held(),
        [receipt.id().clone()],
        "and it was filed once"
    );
    logs.expect_none(
        TARGET,
        "outbox record superseded: another job holds its unique key",
    );
    relay.shutdown().await;
    app.close().await;
}

#[tokio::test]
async fn a_relay_stopped_between_its_call_and_its_settle_leaves_a_record_the_next_files_no_second_time()
 {
    a_relay_restarted_mid_pass(None).await;
}

#[tokio::test]
async fn a_keyed_record_a_restarted_relay_sends_again_holds_its_own_key_and_is_superseded_by_no_one()
 {
    a_relay_restarted_mid_pass(Some("once")).await;
}

#[tokio::test]
async fn a_record_whose_key_another_job_holds_is_superseded_in_one_info_line() {
    let logs = LogCapture::install();
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    let holder = committed_note(&app, &filing, Some("song")).await;
    let superseded = committed_note(&app, &filing, Some("song")).await;
    let relay = Relay::of(&app).await;

    wait_for(PATIENCE, || async {
        kept(&db.connection()).await.is_empty()
    })
    .await;
    assert_eq!(filing.held(), [holder.id().clone()]);
    let line = logs.expect_one(
        TARGET,
        "outbox record superseded: another job holds its unique key",
    );
    assert_eq!(line.level, "info");
    assert_eq!(line.field("job_id"), Some(superseded.id().to_string()));
    assert_eq!(line.field("holder"), Some(holder.id().to_string()));
    relay.shutdown().await;
    app.close().await;
}

#[tokio::test]
async fn a_backend_refusing_every_call_keeps_the_records_and_says_so_at_warn_then_error() {
    let logs = LogCapture::install();
    let db = database().await;
    let filing = Arc::new(Filing::default());
    filing.refusing.store(true, Ordering::SeqCst);
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    arm(&app).await;
    committed_note(&app, &filing, None).await;
    let relay = Relay::of(&app).await;

    const STALLED: &str = "outbox records have waited past the stall warning unfiled; the store \
                           grows until the backend files them";
    wait_until(PATIENCE, || !logs.find(TARGET, STALLED).is_empty()).await;
    assert_eq!(kept(&db.connection()).await.len(), 1, "nothing is lost");
    let attempts: i32 = db
        .connection()
        .query_one_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT attempts FROM nestrs_outbox",
        ))
        .await
        .expect("read")
        .expect("a row")
        .try_get("", "attempts")
        .expect("an integer");
    assert!(attempts >= 1, "each failed pass is counted: {attempts}");
    let warned = logs.find(
        TARGET,
        "outbox records not filed; each is due again after its wait",
    );
    assert!(!warned.is_empty());
    assert_eq!(warned[0].level, "warn");
    assert_eq!(logs.find(TARGET, STALLED).len(), 1);
    relay.shutdown().await;
    app.close().await;
}

#[tokio::test]
async fn two_relays_on_one_table_never_file_one_record_twice() {
    let db = database().await;
    let filing = Arc::new(Filing::default());
    let config = OutboxConfig {
        sweep_interval: Duration::from_millis(100),
        batch_size: 10,
        ..OutboxConfig::default()
    };
    let replicas = [
        TestApp::builder()
            .provide_arc(db.connection())
            .provide_dyn::<dyn JobProducer>(Arc::clone(&filing) as Arc<dyn JobProducer>)
            .provide(Pushing::then_answer(
                &filing,
                &Arc::default(),
                1,
                StatusCode::OK,
            ))
            .provide(config.clone())
            .module::<OutboxAppModule>()
            .build()
            .await
            .expect("the first replica boots"),
        TestApp::builder()
            .provide_arc(db.connection())
            .provide_dyn::<dyn JobProducer>(Arc::clone(&filing) as Arc<dyn JobProducer>)
            .provide(Pushing::then_answer(
                &filing,
                &Arc::default(),
                1,
                StatusCode::OK,
            ))
            .provide(config)
            .module::<OutboxAppModule>()
            .build()
            .await
            .expect("the second replica boots"),
    ];
    arm(&replicas[0]).await;
    in_a_unit(&replicas[0], {
        let filing = Arc::clone(&filing);
        async move {
            filing
                .push_many(NoteQueue, (0..200).map(|n| Note { n }), None)
                .await
                .expect("kept");
            Ok(())
        }
    })
    .await
    .expect("committed");
    let relays = [Relay::of(&replicas[0]).await, Relay::of(&replicas[1]).await];

    wait_for(PATIENCE, || async {
        kept(&db.connection()).await.is_empty()
    })
    .await;
    let handed = filing.handed();
    let mut distinct = handed.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        (handed.len(), distinct.len()),
        (200, 200),
        "every record handed over once, by one relay or the other"
    );
    for relay in relays {
        relay.shutdown().await;
    }
    for replica in replicas {
        replica.close().await;
    }
}

#[tokio::test]
async fn the_boot_refuses_naming_the_remedy_when_the_table_is_missing() {
    let db = EphemeralDatabase::create::<NoMigrations>()
        .await
        .expect("a database of its own, never migrated");
    let filing = Arc::new(Filing::default());
    let app = app_over(
        &db,
        &filing,
        Pushing::then_answer(&filing, &Arc::default(), 1, StatusCode::OK),
    )
    .await;
    let refused = OutboxRelay::new()
        .configure(app.container())
        .await
        .expect_err("the relay cannot arm a store without its table");
    assert_eq!(
        format!("{refused:#}"),
        "the outbox table `nestrs_outbox` is missing: run `nestrs g migration nestrs_outbox`, \
         then `nestrs run db migrate`",
    );
    app.close().await;
}
