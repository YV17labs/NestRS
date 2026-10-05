//! An in-memory queue backend over the port's own seams — the worker's loop,
//! proved in process, and the behaviour kit's run against a backend whose every
//! state a test can read. It keeps what a real backend keeps: a ready list, the
//! deliveries held under a lease with their owner and count, the records due
//! later, the dead letters — and fences every write a delivery makes on its
//! owner and count, as the contract asks.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use nest_rs_queue::{
    Ask, Capabilities, Capability, CheckpointStore, Delivery, Disposition, Envelope, JobConsumer,
    JobId, JobProducer, LeaseHold, Prepared, ProcessMethod, PushOptions, QueueBackend, QueueError,
    QueueName, Received, async_trait,
};
use serde_json::Value;
use tokio::sync::Notify;

/// The in-memory backend that files a record due later and keeps checkpoints.
pub(crate) static DELAYING: QueueBackend = QueueBackend::new(
    "memory",
    Capabilities::NONE
        .with(Capability::DelayedPush)
        .with(Capability::Checkpoint),
);

/// The in-memory backend that files nothing due later: the port waits a
/// retry's backoff itself.
pub(crate) static PLAIN: QueueBackend = QueueBackend::new("memory-plain", Capabilities::NONE);

/// One in-memory store, shared by every producer and consumer cloned from it.
#[derive(Clone)]
pub(crate) struct Memory {
    inner: Arc<Inner>,
}

struct Inner {
    backend: &'static QueueBackend,
    lease: Duration,
    state: Mutex<State>,
    arrived: Notify,
    next_owner: AtomicU64,
}

#[derive(Default)]
struct State {
    queues: HashMap<String, Queue>,
    next_entry: u64,
}

#[derive(Default)]
struct Queue {
    ready: VecDeque<Entry>,
    delayed: Vec<(Instant, Entry)>,
    pending: HashMap<u64, Pending>,
    dead: Vec<Dead>,
    checkpoints: HashMap<JobId, Value>,
}

struct Entry {
    id: u64,
    record: Vec<u8>,
    /// How many times this record was handed over.
    count: u32,
}

struct Pending {
    entry: Entry,
    owner: u64,
    deadline: Instant,
}

/// A dead letter, as the store keeps it.
#[derive(Clone, Debug)]
pub(crate) struct Dead {
    /// The record as stored, empty for one the backend refused.
    pub(crate) record: Vec<u8>,
    /// Why the job ended.
    pub(crate) reason: String,
}

/// A delivery's hold on its entry: the owner and the count it was handed over at.
#[derive(Debug)]
pub(crate) struct MemoryLease {
    queue: String,
    entry: u64,
    owner: u64,
    count: u32,
}

impl Memory {
    /// A store declaring `backend`, whose leases last `lease`.
    pub(crate) fn new(backend: &'static QueueBackend, lease: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                lease,
                state: Mutex::default(),
                arrived: Notify::new(),
                next_owner: AtomicU64::new(1),
            }),
        }
    }

    /// A consumer of this store, owning its deliveries under an owner of its own.
    pub(crate) fn consumer(&self) -> MemoryConsumer {
        MemoryConsumer {
            memory: self.clone(),
            owner: self.inner.next_owner.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// File `record` as stored, due now — a record no push of this release wrote.
    pub(crate) fn file_raw(&self, queue: &str, record: Vec<u8>) {
        let mut state = self.lock();
        let entry = state.entry(record);
        state.queue(queue).ready.push_back(entry);
        drop(state);
        self.inner.arrived.notify_waiters();
    }

    /// The dead letters of `queue`.
    pub(crate) fn dead(&self, queue: &str) -> Vec<Dead> {
        self.lock().queue(queue).dead.clone()
    }

    /// Every record `queue` still holds: ready, held or due later.
    pub(crate) fn held(&self, queue: &str) -> usize {
        let mut state = self.lock();
        let queue = state.queue(queue);
        queue.ready.len() + queue.delayed.len() + queue.pending.len()
    }

    /// Make every lease held on `queue` lapse now, its holder and count kept.
    pub(crate) fn lapse(&self, queue: &str) {
        let now = Instant::now();
        for pending in self.lock().queue(queue).pending.values_mut() {
            pending.deadline = now;
        }
        self.inner.arrived.notify_waiters();
    }

    /// Hand every lease held on `queue` to a holder that lapses at once.
    pub(crate) fn take(&self, queue: &str) {
        let now = Instant::now();
        for pending in self.lock().queue(queue).pending.values_mut() {
            pending.owner = 0;
            pending.entry.count += 1;
            pending.deadline = now;
        }
        self.inner.arrived.notify_waiters();
    }

    /// Forget everything `queue` holds.
    pub(crate) fn purge(&self, queue: &str) {
        self.lock().queues.remove(queue);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    fn queue(&mut self, name: &str) -> &mut Queue {
        self.queues.entry(name.to_owned()).or_default()
    }

    fn entry(&mut self, record: Vec<u8>) -> Entry {
        self.next_entry += 1;
        Entry {
            id: self.next_entry,
            record,
            count: 0,
        }
    }
}

impl Queue {
    /// Whether `lease` is still the delivery holding its entry.
    fn holds(&self, lease: &MemoryLease) -> bool {
        self.pending.get(&lease.entry).is_some_and(|pending| {
            pending.owner == lease.owner && pending.entry.count == lease.count
        })
    }
}

#[async_trait]
impl JobProducer for Memory {
    fn backend(&self) -> &'static QueueBackend {
        self.inner.backend
    }

    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        let due = match options.delay() {
            Some(delay) => {
                let now = SystemTime::now();
                let at = delay.deadline(now)?;
                Some(Instant::now() + at.duration_since(now).unwrap_or_default())
            }
            None => None,
        };
        let mut state = self.lock();
        for envelope in envelopes {
            let entry = state.entry(envelope.into_json().to_string().into_bytes());
            let queue = state.queue(queue.as_str());
            match due {
                Some(due) => queue.delayed.push((due, entry)),
                None => queue.ready.push_back(entry),
            }
        }
        drop(state);
        self.inner.arrived.notify_waiters();
        Ok(())
    }
}

/// A consumer of one [`Memory`] store.
pub(crate) struct MemoryConsumer {
    memory: Memory,
    owner: u64,
}

impl MemoryConsumer {
    /// Hand over what is ready or lapsed, up to `max`.
    fn take(&self, queue: &str, max: usize) -> Vec<Delivery<MemoryLease>> {
        let lease = self.memory.inner.lease;
        let now = Instant::now();
        let mut state = self.memory.lock();
        let queue_state = state.queue(queue);
        let mut taken = Vec::new();
        let lapsed: Vec<u64> = queue_state
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| *id)
            .take(max)
            .collect();
        for id in lapsed {
            if let Some(pending) = queue_state.pending.remove(&id) {
                taken.push(pending.entry);
            }
        }
        while taken.len() < max {
            match queue_state.ready.pop_front() {
                Some(entry) => taken.push(entry),
                None => break,
            }
        }
        let mut deliveries = Vec::with_capacity(taken.len());
        for mut entry in taken {
            entry.count += 1;
            let delivery = Delivery::new(
                entry.record.clone(),
                MemoryLease {
                    queue: queue.to_owned(),
                    entry: entry.id,
                    owner: self.owner,
                    count: entry.count,
                },
                lease,
            )
            .with_delivery_count(entry.count)
            .with_backend_id(entry.id.to_string());
            queue_state.pending.insert(
                entry.id,
                Pending {
                    entry,
                    owner: self.owner,
                    deadline: now + lease,
                },
            );
            deliveries.push(delivery);
        }
        deliveries
    }
}

impl JobConsumer for MemoryConsumer {
    type Lease = MemoryLease;

    fn backend(&self) -> &'static QueueBackend {
        self.memory.inner.backend
    }

    async fn prepare(&self, _methods: &[&'static ProcessMethod]) -> Result<Prepared, QueueError> {
        Ok(Prepared::new(Duration::from_millis(100)))
    }

    async fn receive(
        &self,
        method: &'static ProcessMethod,
        ask: Ask,
    ) -> Result<Received<MemoryLease>, QueueError> {
        let max = usize::try_from(ask.max.get()).unwrap_or(usize::MAX);
        let until = Instant::now() + ask.wait;
        loop {
            let arrived = self.memory.inner.arrived.notified();
            let deliveries = self.take(method.queue(), max);
            if !deliveries.is_empty() {
                return Ok(Received::new(deliveries));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Received::new(Vec::new()));
            }
            // A lapse is no arrival: look again at least every 20 ms.
            let _arrived = tokio::time::timeout(left.min(Duration::from_millis(20)), arrived).await;
        }
    }

    async fn renew(
        &self,
        _method: &'static ProcessMethod,
        leases: &[&MemoryLease],
    ) -> Result<Vec<LeaseHold>, QueueError> {
        let deadline = Instant::now() + self.memory.inner.lease;
        let mut state = self.memory.lock();
        Ok(leases
            .iter()
            .map(|lease| {
                let queue = state.queue(&lease.queue);
                if queue.holds(lease) {
                    if let Some(pending) = queue.pending.get_mut(&lease.entry) {
                        pending.deadline = deadline;
                    }
                    LeaseHold::Held
                } else {
                    LeaseHold::Lost
                }
            })
            .collect())
    }

    async fn settle(
        &self,
        _method: &'static ProcessMethod,
        lease: &MemoryLease,
        disposition: Disposition<'_>,
    ) -> Result<LeaseHold, QueueError> {
        let mut state = self.memory.lock();
        if !state.queue(&lease.queue).holds(lease) {
            return Ok(LeaseHold::Lost);
        }
        let ended = state.queue(&lease.queue).pending.remove(&lease.entry);
        let job = ended
            .as_ref()
            .and_then(|pending| serde_json::from_slice::<Value>(&pending.entry.record).ok())
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
            .and_then(|id| JobId::parse(&id).ok());
        let refile = |state: &mut State, record: &[u8], after: Duration| {
            let entry = state.entry(record.to_vec());
            let queue = state.queue(&lease.queue);
            if after.is_zero() {
                queue.ready.push_back(entry);
            } else {
                queue.delayed.push((Instant::now() + after, entry));
            }
        };
        match disposition {
            Disposition::Complete => {
                if let Some(job) = job {
                    state.queue(&lease.queue).checkpoints.remove(&job);
                }
            }
            Disposition::Retry { after, record, .. } | Disposition::Defer { after, record, .. } => {
                refile(&mut state, record, after);
            }
            Disposition::Requeue { record, .. } => refile(&mut state, record, Duration::ZERO),
            Disposition::DeadLetter { reason, record, .. } => {
                let queue = state.queue(&lease.queue);
                queue.dead.push(Dead {
                    record: record.to_vec(),
                    reason: reason.to_owned(),
                });
                if let Some(job) = job {
                    queue.checkpoints.remove(&job);
                }
            }
            other => panic!("a disposition the in-memory backend was not written for: {other:?}"),
        }
        drop(state);
        self.memory.inner.arrived.notify_waiters();
        Ok(LeaseHold::Held)
    }

    async fn maintain(
        &self,
        method: &'static ProcessMethod,
    ) -> Result<Option<Duration>, QueueError> {
        let now = Instant::now();
        let mut state = self.memory.lock();
        let queue = state.queue(method.queue());
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut queue.delayed)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        queue.delayed = later;
        let moved = !due.is_empty();
        queue.ready.extend(due.into_iter().map(|(_, entry)| entry));
        drop(state);
        if moved {
            self.memory.inner.arrived.notify_waiters();
        }
        Ok(Some(Duration::from_millis(10)))
    }

    fn checkpoint(
        &self,
        method: &'static ProcessMethod,
        lease: &MemoryLease,
        job: &JobId,
    ) -> Option<Arc<dyn CheckpointStore>> {
        Some(Arc::new(MemoryCheckpoint {
            memory: self.memory.clone(),
            queue: method.queue().to_owned(),
            job: job.clone(),
            lease: MemoryLease {
                queue: lease.queue.clone(),
                entry: lease.entry,
                owner: lease.owner,
                count: lease.count,
            },
        }))
    }
}

/// One job's checkpoint, saved only while its delivery still holds the job.
struct MemoryCheckpoint {
    memory: Memory,
    queue: String,
    job: JobId,
    lease: MemoryLease,
}

#[async_trait]
impl CheckpointStore for MemoryCheckpoint {
    async fn load(&self) -> Result<Option<Value>, QueueError> {
        Ok(self
            .memory
            .lock()
            .queue(&self.queue)
            .checkpoints
            .get(&self.job)
            .cloned())
    }

    async fn save(&self, state: Value) -> Result<(), QueueError> {
        let mut store = self.memory.lock();
        let queue = store.queue(&self.queue);
        if !queue.holds(&self.lease) {
            return Err(QueueError::backend(std::io::Error::other(
                "the delivery saving this checkpoint no longer holds its job",
            )));
        }
        queue.checkpoints.insert(self.job.clone(), state);
        Ok(())
    }

    async fn clear(&self) -> Result<(), QueueError> {
        self.memory
            .lock()
            .queue(&self.queue)
            .checkpoints
            .remove(&self.job);
        Ok(())
    }
}
