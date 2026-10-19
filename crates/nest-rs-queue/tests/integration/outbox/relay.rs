//! The relay over an in-memory store and a backend that records what it
//! files: what a committed unit wrote is filed once, what fails is kept and
//! said, and the way down cuts no call. The store's SQL is proved against
//! Postgres in `nest-rs-seaorm`'s e2e.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use nest_rs_core::{Container, Transport};
use nest_rs_database::{Executor as DatabaseExecutor, OutboxError, OutboxRecord, OutboxStore};
use nest_rs_queue::{
    BACKEND_TIMEOUT, Capabilities, Capability, Delay, Envelope, JobId, JobProducer, JobProducerExt,
    OutboxConfig, OutboxRelay, PushOptions, PushReceipt, QueueBackend, QueueError, QueueName,
    TARGET, async_trait,
};
use nest_rs_testing::{LogCapture, wait_until};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::outbox_store::MemoryOutbox;
use crate::{SyncCommand, SyncQueue, TranscodeCommand, TranscodeQueue};

/// A backend filing delayed and keyed jobs.
static FILING: QueueBackend = QueueBackend::new(
    "filing",
    Capabilities::NONE
        .with(Capability::DelayedPush)
        .with(Capability::UniquePush),
);

/// How long a test waits for the relay, on a paused clock.
const PATIENCE: Duration = Duration::from_secs(60);

/// A backend that keeps every job it files, as the contract asks: an envelope
/// whose job it holds is skipped, a key another job holds is refused.
#[derive(Default)]
struct Filing {
    /// Every call: the queue, the ids handed over, the options.
    calls: Mutex<Vec<(String, Vec<JobId>, PushOptions)>>,
    /// Every job it holds: queue, id, unique key.
    held: Mutex<Vec<(String, JobId, Option<String>)>>,
    refusing: AtomicBool,
    /// Whether a call files, then never answers.
    silent_after_filing: AtomicBool,
    /// Whether a call waits for `opened` before it files.
    gated: AtomicBool,
    opened: Notify,
    /// Notified each time a call has filed.
    filed: Notify,
}

impl Filing {
    fn calls(&self) -> Vec<(String, Vec<JobId>, PushOptions)> {
        self.calls.lock().expect("lock").clone()
    }

    fn held(&self) -> Vec<JobId> {
        self.held
            .lock()
            .expect("lock")
            .iter()
            .map(|(_, id, _)| id.clone())
            .collect()
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
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        if self.gated.load(Ordering::SeqCst) {
            self.opened.notified().await;
        }
        if self.refusing.load(Ordering::SeqCst) {
            return Err(QueueError::backend(std::io::Error::other(
                "connection refused",
            )));
        }
        self.calls.lock().expect("lock").push((
            queue.to_string(),
            envelopes
                .iter()
                .map(|envelope| envelope.id().clone())
                .collect(),
            options.clone(),
        ));
        {
            let mut held = self.held.lock().expect("lock");
            for envelope in &envelopes {
                if held.iter().any(|(_, id, _)| id == envelope.id()) {
                    continue;
                }
                if let Some(key) = envelope.unique_key()
                    && let Some((_, holder, _)) = held.iter().find(|(on, _, holding)| {
                        on == queue.as_str() && holding.as_deref() == Some(key)
                    })
                {
                    return Err(QueueError::UniqueKeyHeld {
                        queue: queue.clone(),
                        key: key.to_owned(),
                        holder: holder.clone(),
                    });
                }
                held.push((
                    queue.to_string(),
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
}

/// A unit's handle keeping an outbox: it keeps what a push writes, for the
/// test to commit.
#[derive(Default)]
struct Keeping(Mutex<Vec<OutboxRecord>>);

impl DatabaseExecutor for Keeping {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn write_outbox(
        &self,
        records: &mut dyn Iterator<Item = OutboxRecord>,
    ) -> Option<BoxFuture<'_, Result<(), OutboxError>>> {
        self.0.lock().expect("lock").extend(records);
        Some(Box::pin(std::future::ready(Ok(()))))
    }
}

/// The records the pushes `push` makes write, through the port's own push.
async fn written<F, T>(push: impl FnOnce(Arc<Filing>) -> F) -> (Vec<OutboxRecord>, T)
where
    F: Future<Output = T>,
{
    let keeping = Arc::new(Keeping::default());
    let answer =
        nest_rs_database::with_job_executor(keeping.clone(), push(Arc::new(Filing::default())))
            .await;
    let records = std::mem::take(&mut *keeping.0.lock().expect("lock"));
    (records, answer)
}

/// The records of one push of `file` onto `transcode`, and its receipt.
async fn one_transcode(file: &str, options: PushOptions) -> (Vec<OutboxRecord>, PushReceipt) {
    let (records, receipt) = written(|producer| async move {
        producer
            .push(
                TranscodeQueue,
                TranscodeCommand {
                    file: file.to_owned(),
                },
                options,
            )
            .await
    })
    .await;
    (records, receipt.expect("written to the outbox"))
}

fn settings() -> OutboxConfig {
    OutboxConfig {
        sweep_interval: Duration::from_millis(100),
        ..OutboxConfig::default()
    }
}

/// A container holding the store and the backend the relay drains.
fn container(store: &MemoryOutbox, producer: &Arc<Filing>, config: OutboxConfig) -> Container {
    Container::builder()
        .provide_dyn::<dyn OutboxStore>(Arc::new(store.clone()))
        .provide_dyn::<dyn JobProducer>(Arc::clone(producer) as Arc<dyn JobProducer>)
        .provide(config)
        .build()
        .expect("the container builds")
}

/// A relay configured over `container` and serving until `stop` fires.
struct Serving {
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Serving {
    async fn start(container: &Container) -> Self {
        let mut relay = OutboxRelay::new();
        relay
            .configure(container)
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

#[tokio::test(start_paused = true)]
async fn a_job_a_committed_unit_wrote_is_filed_once_under_its_receipt_and_its_record_deleted() {
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    let relay = Serving::start(&container(&store, &producer, settings())).await;

    let (records, receipt) = one_transcode("song.wav", PushOptions::default()).await;
    assert!(
        producer.calls().is_empty(),
        "nothing is filed before the commit"
    );
    store.commit(records);

    wait_until(PATIENCE, || store.kept().is_empty()).await;
    let calls = producer.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "transcode");
    assert_eq!(
        calls[0].1,
        [receipt.id().clone()],
        "filed under the receipt's id"
    );
    assert_eq!(calls[0].2, PushOptions::default());
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn records_share_a_call_per_queue_and_delay_and_a_keyed_one_has_its_own() {
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    let (mut records, receipts) = written(|producer| async move {
        let many = producer
            .push_many(
                TranscodeQueue,
                ["a.wav", "b.wav", "c.wav"].map(|file| TranscodeCommand { file: file.into() }),
                None,
            )
            .await
            .expect("written");
        producer
            .push(SyncQueue, SyncCommand { org: "acme".into() }, None)
            .await
            .expect("written");
        producer
            .push(
                TranscodeQueue,
                TranscodeCommand {
                    file: "d.wav".into(),
                },
                PushOptions::default().with_unique("d"),
            )
            .await
            .expect("written");
        producer
            .push(
                TranscodeQueue,
                TranscodeCommand {
                    file: "e.wav".into(),
                },
                PushOptions::default().with_delay(Duration::from_secs(60)),
            )
            .await
            .expect("written");
        many
    })
    .await;
    assert_eq!(records.len(), 6);
    store.commit(records.drain(..));
    let relay = Serving::start(&container(&store, &producer, settings())).await;

    wait_until(PATIENCE, || store.kept().is_empty()).await;
    let calls = producer.calls();
    let shapes: Vec<(&str, usize, Option<Delay>, Option<&str>)> = calls
        .iter()
        .map(|(queue, ids, options)| {
            (
                queue.as_str(),
                ids.len(),
                options.delay(),
                options.unique_key(),
            )
        })
        .collect();
    assert_eq!(
        shapes,
        [
            ("transcode", 3, None, None),
            ("sync", 1, None, None),
            ("transcode", 1, None, Some("d")),
            (
                "transcode",
                1,
                Some(Delay::For(Duration::from_secs(60))),
                None
            ),
        ],
        "oldest first, a delay handed on whole when nothing of it has passed"
    );
    assert_eq!(
        calls[0].1,
        receipts
            .iter()
            .map(|receipt| receipt.id().clone())
            .collect::<Vec<_>>()
    );
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_delay_is_handed_on_as_what_is_left_of_it() {
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    let (records, _) = one_transcode(
        "later.wav",
        PushOptions::default().with_delay(Duration::from_secs(60)),
    )
    .await;
    store.commit(records);
    tokio::time::sleep(Duration::from_secs(20)).await;
    let relay = Serving::start(&container(&store, &producer, settings())).await;

    wait_until(PATIENCE, || store.kept().is_empty()).await;
    assert_eq!(
        producer.calls()[0].2.delay(),
        Some(Delay::For(Duration::from_secs(40)))
    );
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_record_whose_key_another_job_holds_is_superseded_in_one_line_naming_both_jobs() {
    let logs = LogCapture::install();
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    let (first, holder) =
        one_transcode("first.wav", PushOptions::default().with_unique("song")).await;
    let (second, superseded) =
        one_transcode("second.wav", PushOptions::default().with_unique("song")).await;
    store.commit(first);
    store.commit(second);
    let relay = Serving::start(&container(&store, &producer, settings())).await;

    wait_until(PATIENCE, || store.kept().is_empty()).await;
    assert_eq!(
        producer.held(),
        [holder.id().clone()],
        "one job holds the key"
    );
    let line = logs.expect_one(
        TARGET,
        "outbox record superseded: another job holds its unique key",
    );
    assert_eq!(line.level, "info");
    assert_eq!(line.field("queue").as_deref(), Some("transcode"));
    assert_eq!(
        line.field("job_id"),
        Some(superseded.id().to_string()),
        "the record's job"
    );
    assert_eq!(
        line.field("holder"),
        Some(holder.id().to_string()),
        "the job holding the key"
    );
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_backend_refusing_every_call_keeps_the_records_warns_each_pass_and_errors_once_past_the_stall()
 {
    let logs = LogCapture::install();
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    producer.refusing.store(true, Ordering::SeqCst);
    let config = OutboxConfig {
        stall_warning: Duration::from_secs(1),
        ..settings()
    };
    let relay = Serving::start(&container(&store, &producer, config)).await;
    let (first, _) = one_transcode("a.wav", PushOptions::default()).await;
    let (second, _) = one_transcode("b.wav", PushOptions::default()).await;
    store.commit(first.into_iter().chain(second));

    const STALLED: &str = "outbox records have waited past the stall warning unfiled; the store \
                           grows until the backend files them";
    wait_until(PATIENCE, || !logs.find(TARGET, STALLED).is_empty()).await;
    wait_until(PATIENCE, || {
        store.kept().iter().all(|kept| kept.attempts >= 3)
    })
    .await;

    let kept = store.kept();
    assert_eq!(kept.len(), 2, "nothing is lost while the backend refuses");
    assert!(
        kept.iter().all(|kept| !kept.due_again_in.is_zero()),
        "each waits its backoff: {kept:?}"
    );
    let warned = logs.find(
        TARGET,
        "outbox records not filed; each is due again after its wait",
    );
    assert!(warned.len() >= 3, "one line per pass: {}", warned.len());
    assert_eq!(
        warned[0].field("count").as_deref(),
        Some("2"),
        "the first pass claimed both; each then waits a backoff of its own"
    );
    for line in &warned {
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("queue").as_deref(), Some("transcode"));
        assert!(
            matches!(line.field("count").as_deref(), Some("1" | "2")),
            "{line:?}"
        );
        assert!(
            line.field("error")
                .is_some_and(|error| error.contains("connection refused")),
            "{line:?}"
        );
    }
    let stalled = logs.find(TARGET, STALLED);
    assert_eq!(stalled.len(), 1, "said once per outage");
    assert_eq!(stalled[0].level, "error");

    producer.refusing.store(false, Ordering::SeqCst);
    wait_until(PATIENCE, || store.kept().is_empty()).await;
    assert_eq!(producer.held().len(), 2);
    logs.expect_one(TARGET, "outbox records are filed again");
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_call_the_backend_never_answers_is_due_again_at_the_ports_net() {
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    producer.gated.store(true, Ordering::SeqCst);
    let relay = Serving::start(&container(&store, &producer, settings())).await;
    let (records, _) = one_transcode("a.wav", PushOptions::default()).await;
    let started = tokio::time::Instant::now();
    store.commit(records);

    wait_until(PATIENCE, || {
        store.kept().first().is_some_and(|kept| kept.attempts == 1)
    })
    .await;
    assert!(started.elapsed() >= BACKEND_TIMEOUT);
    assert!(producer.held().is_empty());
    relay.shutdown().await;
}

/// A relay killed after its call filed the job, before its settle, then
/// another draining the store: the record pushed with `options` is sent
/// again, answered `Ok` and deleted, and the backend holds the job once.
async fn a_relay_restarted_mid_pass(options: PushOptions) {
    let logs = LogCapture::install();
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    producer.silent_after_filing.store(true, Ordering::SeqCst);
    let (records, receipt) = one_transcode("once.wav", options).await;
    store.commit(records);
    let filed = producer.filed.notified();
    let killed = Serving::start(&container(&store, &producer, settings())).await;
    filed.await;
    killed.task.abort();
    assert!(
        killed.task.await.is_err_and(|ended| ended.is_cancelled()),
        "the relay died mid-pass"
    );
    assert_eq!(store.kept().len(), 1, "its record was never settled");

    producer.silent_after_filing.store(false, Ordering::SeqCst);
    let relay = Serving::start(&container(&store, &producer, settings())).await;
    wait_until(PATIENCE, || store.kept().is_empty()).await;
    assert_eq!(
        producer.calls().len(),
        2,
        "the next relay sent the record again"
    );
    assert_eq!(
        producer.held(),
        [receipt.id().clone()],
        "and the backend filed the job once"
    );
    logs.expect_none(
        TARGET,
        "outbox record superseded: another job holds its unique key",
    );
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_relay_stopped_between_its_call_and_its_settle_leaves_a_record_the_next_files_no_second_time()
 {
    a_relay_restarted_mid_pass(PushOptions::default()).await;
}

#[tokio::test(start_paused = true)]
async fn a_keyed_record_sent_again_after_a_relay_stopped_mid_pass_holds_its_own_key_and_is_superseded_by_no_one()
 {
    a_relay_restarted_mid_pass(PushOptions::default().with_unique("once")).await;
}

#[tokio::test(start_paused = true)]
async fn the_way_down_lets_the_call_in_flight_answer_and_starts_no_other() {
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    producer.gated.store(true, Ordering::SeqCst);
    let (transcode, _) = one_transcode("a.wav", PushOptions::default()).await;
    let (sync, _) = written(|producer| async move {
        producer
            .push(SyncQueue, SyncCommand { org: "acme".into() }, None)
            .await
            .expect("written")
    })
    .await;
    store.commit(transcode.into_iter().chain(sync));
    let relay = Serving::start(&container(&store, &producer, settings())).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    relay.stop.cancel();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!relay.task.is_finished(), "the call in flight is never cut");
    producer.gated.store(false, Ordering::SeqCst);
    producer.opened.notify_one();
    relay
        .task
        .await
        .expect("the relay's task ends")
        .expect("the relay stops cleanly");

    let calls = producer.calls();
    assert_eq!(calls.len(), 1, "no call starts on the way down: {calls:?}");
    assert_eq!(calls[0].0, "transcode");
    let kept = store.kept();
    assert_eq!(kept.len(), 1, "what it filed is settled, the rest left");
    assert_eq!(
        kept[0].attempts, 0,
        "the record it never sent counts no failure"
    );
}

#[tokio::test(start_paused = true)]
async fn a_pass_still_running_the_ports_net_after_the_stop_is_cut_and_leaves_its_records_to_the_next_relay()
 {
    let logs = LogCapture::install();
    let store = MemoryOutbox::new();
    store.settle_silently();
    let producer = Arc::new(Filing::default());
    producer.gated.store(true, Ordering::SeqCst);
    let (records, _) = one_transcode("a.wav", PushOptions::default()).await;
    store.commit(records);
    let relay = Serving::start(&container(&store, &producer, settings())).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let stopped = tokio::time::Instant::now();
    relay.stop.cancel();
    relay
        .task
        .await
        .expect("the relay's task ends")
        .expect("the relay stops cleanly");
    let took = stopped.elapsed();
    assert!(
        took >= BACKEND_TIMEOUT && took <= OutboxRelay::new().stop_bound(),
        "cut the port's net after the stop, within its stop bound: {took:?}"
    );
    let line = logs.expect_one(
        TARGET,
        "outbox pass stopped on the way down before it settled; the next relay files its \
         records again",
    );
    assert_eq!(line.level, "warn");
    assert_eq!(
        line.field("net_ms"),
        Some(BACKEND_TIMEOUT.as_millis().to_string())
    );
    let kept = store.kept();
    assert_eq!(kept.len(), 1, "the record stays in the store");
    assert_eq!(kept[0].attempts, 0, "never settled, so no failure counted");
}

#[tokio::test(start_paused = true)]
async fn a_relay_without_a_store_or_a_backend_idles_until_the_stop() {
    let store = MemoryOutbox::new();
    for container in [
        Container::builder()
            .provide_dyn::<dyn OutboxStore>(Arc::new(store.clone()))
            .build()
            .expect("the container builds"),
        Container::builder()
            .provide_dyn::<dyn JobProducer>(Arc::new(Filing::default()) as Arc<dyn JobProducer>)
            .build()
            .expect("the container builds"),
    ] {
        let relay = Serving::start(&container).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!relay.task.is_finished(), "an early end would stop the app");
        relay.shutdown().await;
    }
    assert!(!store.is_armed(), "a store no relay drains is never armed");
}

#[tokio::test(start_paused = true)]
async fn a_store_without_its_table_refuses_the_boot_at_once_naming_the_remedy() {
    let logs = LogCapture::install();
    let store = MemoryOutbox::without_table();
    let mut relay = OutboxRelay::new();
    let started = tokio::time::Instant::now();
    let refused = relay
        .configure(&container(
            &store,
            &Arc::new(Filing::default()),
            OutboxConfig::default(),
        ))
        .await
        .expect_err("the boot stops");
    assert_eq!(
        started.elapsed(),
        Duration::ZERO,
        "what every attempt would meet is never retried"
    );
    let said = format!("{refused:#}");
    assert!(
        said.contains("the outbox table `nestrs_outbox` is missing: create it"),
        "{said}"
    );
    assert!(!store.is_armed());
    logs.expect_none(
        TARGET,
        "outbox store failed arming; retrying within its budget",
    );
}

#[tokio::test(start_paused = true)]
async fn a_store_failing_to_arm_is_retried_within_its_budget_and_said() {
    let logs = LogCapture::install();
    let store = MemoryOutbox::new();
    store.fail_arming(1);
    let started = tokio::time::Instant::now();
    let relay = Serving::start(&container(&store, &Arc::new(Filing::default()), settings())).await;
    let waited = started.elapsed();
    assert!(store.is_armed(), "the second attempt armed it");
    assert!(
        !waited.is_zero() && waited < BACKEND_TIMEOUT,
        "one reconnect wait, inside the budget: {waited:?}"
    );
    let retried = logs.expect_one(
        TARGET,
        "outbox store failed arming; retrying within its budget",
    );
    assert_eq!(retried.level, "warn");
    assert_eq!(retried.field("attempt").as_deref(), Some("1"));
    assert!(
        retried
            .field("error")
            .is_some_and(|error| error.contains("connection reset by peer")),
        "{retried:?}"
    );
    assert_eq!(
        logs.expect_one(TARGET, "outbox armed after retrying")
            .field("attempts")
            .as_deref(),
        Some("2")
    );
    relay.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_store_still_failing_to_arm_at_its_budget_refuses_the_boot_with_the_last_failure() {
    let store = MemoryOutbox::new();
    store.fail_arming(u32::MAX);
    let started = tokio::time::Instant::now();
    let refused = OutboxRelay::new()
        .configure(&container(&store, &Arc::new(Filing::default()), settings()))
        .await
        .expect_err("the boot stops");
    assert_eq!(
        started.elapsed(),
        BACKEND_TIMEOUT,
        "the whole budget, no more"
    );
    let said = format!("{refused:#}");
    assert!(
        said.contains("the outbox store could not be armed within")
            && said.contains("connection reset by peer"),
        "{said}"
    );
    assert!(!store.is_armed());
}

#[tokio::test(start_paused = true)]
async fn a_record_holding_no_job_a_relay_can_file_is_deleted_at_error_without_its_text() {
    let logs = LogCapture::install();
    let store = MemoryOutbox::new();
    let producer = Arc::new(Filing::default());
    let (records, _) = one_transcode("a.wav", PushOptions::default()).await;
    let named = records[0].clone();
    let other = uuid::Uuid::now_v7();
    store.commit([
        OutboxRecord::new(
            "transcode",
            other,
            "the secret, no json".to_owned(),
            None,
            Duration::ZERO,
        ),
        OutboxRecord::new(
            "transcode",
            other,
            named.sealed.clone(),
            None,
            Duration::ZERO,
        ),
        OutboxRecord::new(
            "no:such:queue",
            named.id,
            named.sealed,
            None,
            Duration::ZERO,
        ),
    ]);
    let relay = Serving::start(&container(&store, &producer, settings())).await;

    wait_until(PATIENCE, || store.kept().is_empty()).await;
    assert!(
        producer.calls().is_empty(),
        "nothing filed under another job's id"
    );
    let deleted = logs.find(
        TARGET,
        "outbox record holds no job a relay can file; it is deleted",
    );
    assert_eq!(deleted.len(), 3);
    for line in &deleted {
        assert_eq!(line.level, "error");
        let error = line.field("error").unwrap_or_default();
        assert!(!error.contains("secret"), "{error}");
    }
    assert_eq!(deleted[0].field("job_id"), Some(other.to_string()));
    relay.shutdown().await;
}

#[test]
fn the_relay_stops_within_the_ports_net_and_the_settle() {
    assert_eq!(
        OutboxRelay::new().stop_bound(),
        BACKEND_TIMEOUT + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT
    );
}
