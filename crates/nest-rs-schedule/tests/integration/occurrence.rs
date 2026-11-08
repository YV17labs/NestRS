//! The occurrence lock's binding contract, booted the way an app wires it:
//! `ScheduleModule`, a binding declaring `Arc<dyn OccurrenceLock>` with
//! `BACKEND_REMEDY`, and a `#[scheduled]` host declaring `replicas = "one"`.

use std::any::TypeId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{
    AccessGraphError, App, Collecting, ContainerBuilder, Module, Registering, injectable, module,
};
use nest_rs_schedule::{
    BACKEND_REMEDY, Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError, Replicas,
    ScheduleModule, ScheduledMethod, Scheduler, scheduled,
};
use nest_rs_testing::HeadlessApp;

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

/// What `#[every(.., replicas = "one")]` submitted, read rather than retyped.
fn declared() -> &'static ScheduledMethod {
    nest_rs_core::inventory::iter::<ScheduledMethod>()
        .find(|entry| entry.host_type_id() == TypeId::of::<ReplicatedTasks>())
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
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_declared_factory::<Arc<dyn OccurrenceLock>, _, _>(BACKEND_REMEDY, |_| async {
                Ok(Arc::new(RecordingLock) as Arc<dyn OccurrenceLock>)
            })
    }
}

/// A second backend's binding, of the same shape.
struct SecondLockModule;

impl Module for SecondLockModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
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

#[tokio::test(start_paused = true)]
async fn a_job_firing_once_claims_each_occurrence_through_the_bound_lock() {
    let declared = declared();
    assert_eq!(declared.replicas(), Replicas::One);
    assert_eq!(declared.key(), None);

    let app = HeadlessApp::builder()
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
        .origin()
        .split_once("::")
        .map_or(declared.origin(), |(krate, _)| krate);
    let job = format!("{krate}:{}:{}", declared.host(), declared.method());
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

#[tokio::test(start_paused = true)]
async fn a_pinned_job_claims_under_the_key_it_declares() {
    let app = HeadlessApp::builder()
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

#[tokio::test]
async fn two_lock_bindings_fail_the_boot_naming_the_remedy() {
    let refusal = HeadlessApp::builder()
        .module::<ContestedRoot>()
        .build_headless()
        .await
        .err()
        .expect("two lock bindings contest the port")
        .to_string();
    assert!(refusal.contains("OccurrenceLock"), "{refusal}");
    assert!(refusal.contains(BACKEND_REMEDY), "{refusal}");
}

#[tokio::test]
async fn a_job_firing_once_with_no_lock_bound_fails_the_boot_naming_it() {
    let declared = declared();
    let app = HeadlessApp::builder()
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
        refusal.contains(&format!("`{}::{}`", declared.host(), declared.method())),
        "names the job: {refusal}"
    );
    assert!(refusal.contains(BACKEND_REMEDY), "{refusal}");
}

#[expect(
    dead_code,
    reason = "the lock is declared for the container to resolve, never read"
)]
#[injectable]
struct ReadsTheLock {
    #[inject]
    lock: Arc<dyn OccurrenceLock>,
}

#[module(providers = [ReadsTheLock])]
struct LockWithNoBinding;

/// `AppLockModule` below lists the lock in this binary, a module this app does
/// not reach, so the access graph says it: the port's line all the same. With
/// no module listing it, `tests/unlisted` holds the unmet dependency's.
#[tokio::test]
async fn a_provider_injecting_the_lock_with_no_binding_is_told_the_binding() {
    let Err(refused) = App::builder().module::<LockWithNoBinding>().build().await else {
        panic!("no module binds the lock");
    };
    let violation = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not an access violation: {refused:#}"));
    assert_eq!(violation.remedy, Some(BACKEND_REMEDY), "{violation}");
    assert!(
        violation.to_string().ends_with(BACKEND_REMEDY),
        "{violation}"
    );
}

/// A lock the app binds itself, in a module of its own.
#[injectable]
#[derive(Default)]
struct AppLock;

#[async_trait::async_trait]
impl OccurrenceLock for AppLock {
    async fn claim(&self, _: &Occurrence) -> Result<OccurrenceClaim, OccurrenceLockError> {
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, _: &str) -> Result<bool, OccurrenceLockError> {
        Ok(false)
    }
}

#[module(providers = [AppLock as dyn OccurrenceLock])]
struct AppLockModule;

#[module(imports = [AppLockModule, LockWithNoBinding])]
struct LockListedBesideItsReader;

/// The module listing the lock is the fix, whatever the port names: a binding
/// imported beside it would register the key twice.
#[tokio::test]
async fn a_reader_beside_the_module_listing_the_lock_is_told_that_module() {
    let Err(refused) = App::builder()
        .module::<LockListedBesideItsReader>()
        .build()
        .await
    else {
        panic!("the reader imports nothing");
    };
    let violation = refused
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("not an access violation: {refused:#}"));
    assert_eq!(
        (violation.consumer, violation.owner),
        ("ReadsTheLock", "AppLockModule")
    );
    let said = violation.to_string();
    assert!(said.contains("— add `AppLockModule` to"), "{said}");
    assert!(!said.contains(BACKEND_REMEDY), "{said}");
}

#[module(imports = [AppLockModule], providers = [ReadsTheLock])]
struct ReaderOverItsLock;

#[tokio::test]
async fn following_that_sentence_boots_on_the_apps_lock() {
    let app = App::builder()
        .module::<ReaderOverItsLock>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the import is the fix: {refused:#}"));
    assert!(app.container().get::<ReadsTheLock>().is_some());
    assert!(app.container().get::<AppLock>().is_some());
}
