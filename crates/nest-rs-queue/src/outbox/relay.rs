//! [`OutboxRelay`] — the port's transport filing what committed units wrote
//! into their outbox, at least once.
//!
//! **A pass claims, files, then settles.** It locks a batch of due records no
//! other relay holds, hands them to the backend in as few calls as their
//! queues, delays and unique keys allow, then deletes what was filed and makes
//! what failed due again after a wait. A relay dying between a call and the
//! settle leaves its records to the next pass, which sends them again: the
//! backend files an envelope whose job it still holds once
//! ([`JobProducer::enqueue`]), and a handler keys its idempotency on the job's
//! id for a job that already ended.
//!
//! **The way down never cuts a call to the backend.** Once the app stops, the
//! relay starts no call; the one in flight answers within the port's net, and
//! the pass then settles within what is left of it, or leaves its records to
//! the next relay.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nest_rs_core::{
    Attempt, Backoff, Container, Proving, SHUTDOWN_SETTLE_TIMEOUT, Transport, Unanswered, Unproved,
    process_seed, prove, within,
};
use nest_rs_database::{
    ClaimedRecord, OutboxClaim, OutboxError, OutboxRecord, OutboxStore, UnitOfWork,
};
use tokio_util::sync::CancellationToken;

use crate::backend::bounded;
use crate::backoff::retry_after;
use crate::consume::millis;
use crate::envelope::Envelope;
use crate::producer::refuse_what_no_push_may_carry;
use crate::{
    BACKEND_TIMEOUT, ENQUEUE_BATCH, JobId, JobProducer, OutboxConfig, PushOptions, QueueError,
    QueueName, TARGET,
};

/// The transport [`OutboxModule`](crate::OutboxModule) attaches: it files the
/// jobs units wrote into the [`OutboxStore`] a data driver bound, on the
/// [`JobProducer`] a queue backend bound, and idles when either is missing.
///
/// Its boot arms the store through the boot's proof, refusing at once without
/// the store's table and retrying what may clear within the store's net; its
/// stop finishes the call in flight within the port's net, so its
/// [`stop_bound`](Transport::stop_bound) is
/// [`BACKEND_TIMEOUT`] plus the settle.
#[derive(Default)]
pub struct OutboxRelay {
    config: OutboxConfig,
    /// What `serve` drains, once `configure` found a store and a backend.
    ready: Option<Ready>,
}

/// A configured relay's store and backend.
struct Ready {
    store: Arc<dyn OutboxStore>,
    producer: Arc<dyn JobProducer>,
    /// What a call to the store is waited on: its database's settle net.
    net: Duration,
}

impl OutboxRelay {
    /// A relay with nothing to drain yet: the store, the backend and the
    /// settings are read from the container at boot.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Transport for OutboxRelay {
    async fn configure(&mut self, container: &Container) -> anyhow::Result<()> {
        if let Some(config) = container.get::<OutboxConfig>() {
            self.config = (*config).clone();
        }
        let (Some(store), Some(producer)) = (
            container.get_dyn::<dyn OutboxStore>(),
            container.get_dyn::<dyn JobProducer>(),
        ) else {
            return Ok(());
        };
        // The store's statements are its database's, bounded as every unit's.
        let net = container
            .get_dyn::<dyn UnitOfWork>()
            .map_or(BACKEND_TIMEOUT, |unit_of_work| unit_of_work.settle_net());
        arm(store.as_ref(), net).await?;
        tracing::debug!(
            target: TARGET,
            batch_size = self.config.batch_size,
            sweep_interval_ms = millis(self.config.sweep_interval),
            "outbox armed",
        );
        self.ready = Some(Ready {
            store,
            producer,
            net,
        });
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(ready) = self.ready else {
            // Nothing to drain: idle until shutdown, so this transport does not
            // race the app down.
            cancel.cancelled().await;
            return Ok(());
        };
        let mut relaying = Relaying {
            ready,
            config: self.config,
            stalled: false,
        };
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                () = relaying.ready.store.written(relaying.config.sweep_interval) => {}
            }
            // A full batch is followed at once: more may be due behind it.
            while relaying.pass_within_the_stop(&cancel).await && !cancel.is_cancelled() {}
        }
    }

    /// The call in flight when the app stops answers within the port's net,
    /// and the pass settles within what is left of it.
    fn stop_bound(&self) -> Duration {
        BACKEND_TIMEOUT + SHUTDOWN_SETTLE_TIMEOUT
    }
}

/// A serving relay: its store and backend, its settings, and whether it has
/// already said its records are stalled.
struct Relaying {
    ready: Ready,
    config: OutboxConfig,
    stalled: bool,
}

/// What one pass decided for each record it claimed.
#[derive(Default)]
struct Settling {
    /// Filed, superseded, or no job at all: deleted.
    done: Vec<i64>,
    /// Failed: due again after its wait.
    retry: Vec<(i64, Duration)>,
}

/// One call to the backend: records of one queue sharing their options.
struct Call {
    queue: QueueName,
    options: PushOptions,
    envelopes: Vec<Envelope>,
    /// Each envelope's record: its place, its job and its failed attempts.
    owed: Vec<(i64, JobId, u32)>,
}

impl Relaying {
    /// One pass, cut once the app has stopped and the port's net has passed
    /// since — by then no call to the backend is in flight, since none starts
    /// after the stop. Answers whether it claimed a full batch.
    async fn pass_within_the_stop(&mut self, cancel: &CancellationToken) -> bool {
        let stopped = async {
            cancel.cancelled().await;
            tokio::time::sleep(BACKEND_TIMEOUT).await;
        };
        tokio::select! {
            biased;
            full = self.pass(cancel) => full,
            () = stopped => {
                tracing::warn!(
                    target: TARGET,
                    net_ms = millis(BACKEND_TIMEOUT),
                    "outbox pass stopped on the way down before it settled; the next relay files \
                     its records again",
                );
                false
            }
        }
    }

    /// Claim a batch, file it, settle it. Answers whether the batch was full.
    async fn pass(&mut self, cancel: &CancellationToken) -> bool {
        let limit = self.config.batch_size;
        let claimed = tokio::select! {
            biased;
            () = cancel.cancelled() => return false,
            claimed = within(self.ready.net, async { self.ready.store.claim(limit).await }) => claimed,
        };
        let claim = match claimed {
            Ok(Ok(claim)) => claim,
            Ok(Err(error)) => {
                tracing::warn!(
                    target: TARGET,
                    error = %nest_rs_core::error_message(&error),
                    "outbox claim failed; the next pass tries again",
                );
                return false;
            }
            Err(Unanswered::Panicked(payload)) => {
                nest_rs_core::contained_panic!(
                    target: TARGET,
                    payload.as_ref(),
                    "outbox store panicked claiming records; the next pass tries again",
                );
                return false;
            }
            Err(_) => {
                tracing::warn!(
                    target: TARGET,
                    net_ms = millis(self.ready.net),
                    "outbox claim did not answer within its net; the next pass tries again",
                );
                return false;
            }
        };
        let full = claim.records().len() >= limit;
        let mut settling = Settling::default();
        let calls = calls(claim.records(), &mut settling);
        let filed = self
            .file(calls, cancel, &mut settling, claim.records())
            .await;
        if filed && self.stalled {
            tracing::info!(
                target: TARGET,
                count = settling.done.len(),
                "outbox records are filed again",
            );
            self.stalled = false;
        }
        self.settle(claim, settling).await;
        full
    }

    /// Hand every call to the backend, oldest first, starting none once the app
    /// has stopped, and say what failed. Answers whether every call it made
    /// filed and at least one was made.
    async fn file(
        &mut self,
        calls: Vec<Call>,
        cancel: &CancellationToken,
        settling: &mut Settling,
        records: &[ClaimedRecord],
    ) -> bool {
        let mut failed: Vec<(QueueName, usize, String)> = Vec::new();
        let mut made = false;
        for call in calls {
            if cancel.is_cancelled() {
                break;
            }
            made = true;
            let Call {
                queue,
                options,
                envelopes,
                owed,
            } = call;
            match self.enqueue(&queue, envelopes, &options).await {
                Ok(()) => settling.done.extend(owed.iter().map(|(seq, ..)| *seq)),
                Err(QueueError::UniqueKeyHeld { holder, .. }) if options.unique_key().is_some() => {
                    for (seq, id, _) in &owed {
                        tracing::info!(
                            target: TARGET,
                            queue = %queue,
                            job_id = %id,
                            holder = %holder,
                            "outbox record superseded: another job holds its unique key",
                        );
                        settling.done.push(*seq);
                    }
                }
                Err(error) => {
                    for (seq, id, attempts) in &owed {
                        settling
                            .retry
                            .push((*seq, retry_after(id, attempts.saturating_add(1))));
                    }
                    let said = nest_rs_core::error_message(&error);
                    match failed.iter_mut().find(|(on, ..)| *on == queue) {
                        Some((_, count, last)) => {
                            *count += owed.len();
                            *last = said;
                        }
                        None => failed.push((queue, owed.len(), said)),
                    }
                }
            }
        }
        for (queue, count, error) in &failed {
            tracing::warn!(
                target: TARGET,
                queue = %queue,
                count,
                error = %error,
                "outbox records not filed; each is due again after its wait",
            );
        }
        self.say_stalled(&failed, settling, records);
        made && failed.is_empty()
    }

    /// One call to the backend under the port's net, refused first what the
    /// backend no longer declares — a backend swapped since the push.
    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        let producer = &self.ready.producer;
        refuse_what_no_push_may_carry(producer.backend(), options)?;
        bounded(queue, "JobProducer::enqueue", async {
            producer.enqueue(queue, envelopes, options).await
        })
        .await
    }

    /// Say once, at `error`, that records have waited unfiled past the stall
    /// warning: the backend is down, and the store grows.
    fn say_stalled(
        &mut self,
        failed: &[(QueueName, usize, String)],
        settling: &Settling,
        records: &[ClaimedRecord],
    ) {
        if self.stalled || failed.is_empty() {
            return;
        }
        let waited = records
            .iter()
            .filter(|claimed| settling.retry.iter().any(|(seq, _)| *seq == claimed.seq))
            .map(|claimed| claimed.waited)
            .max()
            .unwrap_or_default();
        if waited < self.config.stall_warning {
            return;
        }
        let count: usize = failed.iter().map(|(_, count, _)| count).sum();
        tracing::error!(
            target: TARGET,
            count,
            waited_ms = millis(waited),
            stall_warning_ms = millis(self.config.stall_warning),
            "outbox records have waited past the stall warning unfiled; the store grows until \
             the backend files them",
        );
        self.stalled = true;
    }

    /// Settle what the pass decided, or leave it to the next pass.
    async fn settle(&self, claim: Box<dyn OutboxClaim>, settling: Settling) {
        let settled = within(self.ready.net, async {
            claim.settle(&settling.done, &settling.retry).await
        })
        .await;
        let unsettled = |error: String| {
            tracing::warn!(
                target: TARGET,
                count = settling.done.len() + settling.retry.len(),
                error = %error,
                "outbox claim could not be settled; the next pass files its records again",
            );
        };
        match settled {
            Ok(Ok(())) => {}
            Ok(Err(error)) => unsettled(nest_rs_core::error_message(&error)),
            Err(Unanswered::Panicked(payload)) => {
                nest_rs_core::contained_panic!(
                    target: TARGET,
                    payload.as_ref(),
                    "outbox store panicked settling a claim; the next pass files its records again",
                );
            }
            Err(_) => unsettled(format!(
                "no answer within its net of {}ms",
                millis(self.ready.net)
            )),
        }
    }
}

/// Arm `store` through the boot's proof, within `budget`: a missing table
/// refuses the boot at once, naming the remedy, and a failure that may clear —
/// a connection lost, a server restarting — is retried on the reconnect curve,
/// each said at `warn`.
async fn arm(store: &dyn OutboxStore, budget: Duration) -> anyhow::Result<()> {
    let armed = prove(
        budget,
        Backoff::RECONNECT,
        process_seed(),
        said_arming,
        |left| async move {
            match within(left, async { store.arm().await }).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(missing @ OutboxError::Missing { .. })) => {
                    Err(Attempt::Refused(anyhow::Error::new(missing)))
                }
                Ok(Err(failed)) => Err(Attempt::Failed(failed)),
                Err(Unanswered::Panicked(payload)) => {
                    nest_rs_core::contained_panic!(
                        target: TARGET,
                        payload.as_ref(),
                        "outbox store panicked arming; the boot stops",
                    );
                    Err(Attempt::Refused(anyhow::anyhow!(
                        "the outbox store panicked arming"
                    )))
                }
                Err(_) => Err(Attempt::Refused(unanswered(budget))),
            }
        },
    )
    .await;
    match armed {
        Ok(()) => Ok(()),
        Err(Unproved::Refused(refusal)) => Err(refusal),
        Err(Unproved::Spent {
            attempts,
            last: Some(failed),
        }) => Err(anyhow::Error::new(failed).context(format!(
            "the outbox store could not be armed within {budget:?}, after {attempts} attempts"
        ))),
        Err(Unproved::Spent { last: None, .. }) => Err(unanswered(budget)),
    }
}

/// The boot error of a store that never answered its arming.
fn unanswered(budget: Duration) -> anyhow::Error {
    anyhow::anyhow!("the outbox store did not answer its arming within {budget:?}")
}

/// What arming the store reports, said on the port's target.
fn said_arming(proving: Proving<'_, OutboxError>) {
    match proving {
        Proving::Retrying { attempt, error } => tracing::warn!(
            target: TARGET,
            attempt,
            error = %nest_rs_core::error_message(error),
            "outbox store failed arming; retrying within its budget",
        ),
        Proving::TimedOut { attempt } => tracing::warn!(
            target: TARGET,
            attempt,
            "outbox store did not answer its arming within its budget",
        ),
        Proving::Proved { attempts } if attempts > 1 => tracing::info!(
            target: TARGET,
            attempts,
            "outbox armed after retrying",
        ),
        _ => {}
    }
}

/// The calls that file `records`, oldest first: records of one queue and one
/// delay share a call of up to [`ENQUEUE_BATCH`], a keyed record has a call of
/// its own, and a record holding no job a relay can file is deleted, said at
/// `error`.
fn calls(records: &[ClaimedRecord], settling: &mut Settling) -> Vec<Call> {
    let mut calls: Vec<Call> = Vec::new();
    let mut open: HashMap<(&str, Duration), usize> = HashMap::new();
    for claimed in records {
        let record = &claimed.record;
        let (queue, envelope) = match job(record) {
            Ok(job) => job,
            Err(error) => {
                tracing::error!(
                    target: TARGET,
                    job_id = %record.id,
                    error = %error,
                    "outbox record holds no job a relay can file; it is deleted",
                );
                settling.done.push(claimed.seq);
                continue;
            }
        };
        let owed = (claimed.seq, envelope.id().clone(), claimed.attempts);
        let mut options = PushOptions::default();
        if !record.delay.is_zero() {
            options = options.with_delay(record.delay);
        }
        if let Some(key) = &record.unique_key {
            calls.push(Call {
                queue,
                options: options.with_unique(key.clone()),
                envelopes: vec![envelope],
                owed: vec![owed],
            });
            continue;
        }
        let slot = (record.queue.as_str(), record.delay);
        match open.get(&slot) {
            Some(&at) if calls[at].envelopes.len() < ENQUEUE_BATCH => {
                calls[at].envelopes.push(envelope);
                calls[at].owed.push(owed);
            }
            _ => {
                open.insert(slot, calls.len());
                calls.push(Call {
                    queue,
                    options,
                    envelopes: vec![envelope],
                    owed: vec![owed],
                });
            }
        }
    }
    calls
}

/// The queue and the envelope `record` holds, or why it holds no job a relay
/// can file, said by kind.
fn job(record: &OutboxRecord) -> Result<(QueueName, Envelope), String> {
    let queue = QueueName::new(record.queue.clone())
        .map_err(|refused| nest_rs_core::error_message(&refused))?;
    let envelope =
        Envelope::from_outbox(&record.sealed, record.id).map_err(|unkept| unkept.to_string())?;
    Ok((queue, envelope))
}
