//! The occurrence lock's binding contract, booted the way an app wires it:
//! `ScheduleModule`, a binding declaring `Arc<dyn OccurrenceLock>` with
//! `BACKEND_REMEDY` — one declared factory and nothing else, the whole of what a
//! backend's binding module is — and a `#[scheduled]` host declaring
//! `replicas = "one"`. Each app is built through the harness and its scheduler
//! configured against the container that build produced, so what is asserted is
//! what the imports decide.

use std::any::TypeId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{ContainerBuilder, Imported, Module, injectable, module};
use nest_rs_schedule::{
    BACKEND_REMEDY, Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError, Replicas,
    ScheduleModule, ScheduledMethod, Scheduler, scheduled,
};
use nest_rs_testing::TestApp;

static RUNS: AtomicU64 = AtomicU64::new(0);
static PINNED_RUNS: AtomicU64 = AtomicU64::new(0);
static CLAIMED: Mutex<Vec<Occurrence>> = Mutex::new(Vec::new());

#[injectable]
#[derive(Default)]
pub(crate) struct ReplicatedTasks;

#[scheduled]
impl ReplicatedTasks {
    #[every("100ms", replicas = "one")]
    async fn sweep(&self) -> anyhow::Result<()> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(providers = [ReplicatedTasks])]
struct ReplicatedTasksModule;

/// A job renamed from `billing::InvoiceTasks::close_day`, pinning the identity
/// it had.
#[injectable]
#[derive(Default)]
pub(crate) struct LedgerTasks;

#[scheduled]
impl LedgerTasks {
    #[every("100ms", replicas = "one", key = "billing::InvoiceTasks::close_day")]
    async fn close(&self) -> anyhow::Result<()> {
        PINNED_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[module(providers = [LedgerTasks])]
struct LedgerTasksModule;

/// What `#[every(.., replicas = "one")]` submitted — read rather than retyped,
/// so the identity the assertions expect is the one the decorator declared.
fn declared() -> &'static ScheduledMethod {
    nest_rs_core::inventory::iter::<ScheduledMethod>()
        .find(|entry| (entry.provider_type_id)() == TypeId::of::<ReplicatedTasks>())
        .expect("`#[scheduled]` submitted the host's one trigger")
}

/// Grants every claim, and records each one.
struct RecordingLock;

#[async_trait::async_trait]
impl OccurrenceLock for RecordingLock {
    async fn claim(&self, occurrence: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        CLAIMED.lock().expect("lock").push(occurrence.clone());
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, token: &str) -> Result<bool, OccurrenceLockError> {
        Ok(CLAIMED
            .lock()
            .expect("lock")
            .iter()
            .any(|claimed| claimed.token == token))
    }
}

/// A binding as a backend writes one: a declared factory for the port, carrying
/// the port's remedy, deduplicated like a `#[module]` expansion.
struct RecordingLockModule;

impl Module for RecordingLockModule {
    fn register(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder
            .provide_declared_factory::<Arc<dyn OccurrenceLock>, _, _>(BACKEND_REMEDY, |_| async {
                Ok(Arc::new(RecordingLock) as Arc<dyn OccurrenceLock>)
            })
    }
}

/// A second backend's binding, of the same shape — what an app imports by
/// mistake beside the first.
struct SecondLockModule;

impl Module for SecondLockModule {
    fn register(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder
            .provide_declared_factory::<Arc<dyn OccurrenceLock>, _, _>(BACKEND_REMEDY, |_| async {
                Ok(Arc::new(RecordingLock) as Arc<dyn OccurrenceLock>)
            })
    }
}

#[module(imports = [ScheduleModule, RecordingLockModule, ReplicatedTasksModule])]
struct BoundRoot;

#[module(imports = [ScheduleModule, RecordingLockModule, LedgerTasksModule])]
struct PinnedRoot;

#[module(imports = [ScheduleModule, RecordingLockModule, SecondLockModule, ReplicatedTasksModule])]
struct ContestedRoot;

#[module(imports = [ScheduleModule, ReplicatedTasksModule])]
struct UnboundRoot;

/// The documented wiring runs: the decorator's `replicas = "one"` reaches the
/// scheduler, the binding's lock is the one it claims through, every claim is
/// made under the job's identity — its crate, the provider and the method, a
/// level each — and an instant on a multiple of the period, each one recording
/// its own run, and each occurrence the lock granted fired once.
///
/// The crate is what keeps two apps of one deployment apart: keyed on
/// `Provider:method` alone, an API's and a worker's own `MaintenanceTasks::sweep`
/// claimed each other's occurrences through the lock they share.
#[tokio::test(start_paused = true)]
async fn a_job_firing_once_claims_each_occurrence_through_the_bound_lock() {
    let declared = declared();
    assert_eq!(declared.replicas, Replicas::One);
    assert_eq!(declared.key, None);

    let app = TestApp::builder()
        .module::<BoundRoot>()
        .build_headless()
        .await
        .expect("an app importing one lock binding boots");
    let scheduler = app
        .spawn_transport(Scheduler::new())
        .await
        .expect("the scheduler configures: a lock is bound");
    tokio::time::sleep(Duration::from_millis(450)).await;
    scheduler.shutdown().await.expect("clean shutdown");

    let claimed = CLAIMED.lock().expect("lock").clone();
    assert!(
        claimed.len() >= 3,
        "a 100 ms job claimed its occurrences: {claimed:?}"
    );
    assert_eq!(
        RUNS.load(Ordering::SeqCst) as usize,
        claimed.len(),
        "each granted occurrence fired, once",
    );
    let krate = declared
        .origin
        .split_once("::")
        .map_or(declared.origin, |(krate, _)| krate);
    let job = format!("{krate}:{}:{}", declared.provider, declared.method);
    assert_eq!(job, "integration:ReplicatedTasks:sweep");
    for occurrence in &claimed {
        let instant: u64 = occurrence
            .token
            .strip_prefix(&format!("{job}:"))
            .and_then(|instant| instant.parse().ok())
            .unwrap_or_else(|| panic!("a claim is `{job}:<instant>`: {occurrence:?}"));
        assert_eq!(instant % 100, 0, "ticks on the epoch: {occurrence:?}");
    }
    let runs: std::collections::HashSet<&str> = claimed
        .iter()
        .map(|occurrence| occurrence.run.as_str())
        .collect();
    assert_eq!(runs.len(), claimed.len(), "each claim records its own run");
}

/// `key = "…"` reaches the lock verbatim, a level per `::`: the job renamed
/// from `billing::InvoiceTasks::close_day` claims under the identity it had, so
/// replicas built before the rename and after it claim the same occurrences.
#[tokio::test(start_paused = true)]
async fn a_pinned_job_claims_under_the_key_it_declares() {
    let app = TestApp::builder()
        .module::<PinnedRoot>()
        .build_headless()
        .await
        .expect("an app importing one lock binding boots");
    let scheduler = app
        .spawn_transport(Scheduler::new())
        .await
        .expect("the scheduler configures: a lock is bound");
    tokio::time::sleep(Duration::from_millis(350)).await;
    scheduler.shutdown().await.expect("clean shutdown");

    let claimed = CLAIMED.lock().expect("lock").clone();
    assert!(claimed.len() >= 2, "{claimed:?}");
    assert_eq!(PINNED_RUNS.load(Ordering::SeqCst) as usize, claimed.len());
    for occurrence in &claimed {
        assert!(
            occurrence
                .token
                .strip_prefix("billing:InvoiceTasks:close_day:")
                .is_some_and(|instant| instant.parse::<u64>().is_ok()),
            "claimed under the pinned identity, never `LedgerTasks::close`: {occurrence:?}"
        );
    }
}

/// Two bindings for one port are two deliberate declarations, and the boot
/// refuses to pick between them — naming the port and the one remedy every
/// binding shares.
#[tokio::test]
async fn two_lock_bindings_fail_the_boot_naming_the_remedy() {
    let refusal = TestApp::builder()
        .module::<ContestedRoot>()
        .build_headless()
        .await
        .err()
        .expect("two lock bindings contest the port")
        .to_string();
    assert!(refusal.contains("OccurrenceLock"), "{refusal}");
    assert!(refusal.contains(BACKEND_REMEDY), "{refusal}");
}

/// A job firing once with nothing to claim through fails the boot — at the
/// earliest site that sees both facts, the scheduler's configure — naming the
/// job and the remedy, rather than firing on every replica or on none.
#[tokio::test]
async fn a_job_firing_once_with_no_lock_bound_fails_the_boot_naming_it() {
    let declared = declared();
    let app = TestApp::builder()
        .module::<UnboundRoot>()
        .build_headless()
        .await
        .expect("the container builds: the scheduler is what refuses");
    let refusal = app
        .spawn_transport(Scheduler::new())
        .await
        .err()
        .expect("no lock is bound")
        .to_string();
    assert!(
        refusal.contains(&format!("`{}::{}`", declared.provider, declared.method)),
        "names the job: {refusal}"
    );
    assert!(refusal.contains(BACKEND_REMEDY), "{refusal}");
}
