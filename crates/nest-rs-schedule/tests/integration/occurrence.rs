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

use nest_rs_core::{ContainerBuilder, Module, injectable, module};
use nest_rs_schedule::{
    BACKEND_REMEDY, OccurrenceLock, OccurrenceLockError, Replicas, ScheduleModule, ScheduledMethod,
    Scheduler, scheduled,
};
use nest_rs_testing::TestApp;

static RUNS: AtomicU64 = AtomicU64::new(0);
static CLAIMED: Mutex<Vec<String>> = Mutex::new(Vec::new());

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

/// What `#[every(.., replicas = "one")]` submitted — read rather than retyped,
/// so the identity the assertions expect is the one the decorator declared.
fn declared() -> &'static ScheduledMethod {
    nest_rs_core::inventory::iter::<ScheduledMethod>()
        .find(|entry| (entry.provider_type_id)() == TypeId::of::<ReplicatedTasks>())
        .expect("`#[scheduled]` submitted the host's one trigger")
}

/// Grants every claim, and records the token each was made under.
struct RecordingLock;

#[async_trait::async_trait]
impl OccurrenceLock for RecordingLock {
    async fn claim(&self, occurrence: &str, _hold: Duration) -> Result<bool, OccurrenceLockError> {
        CLAIMED.lock().expect("lock").push(occurrence.to_owned());
        Ok(true)
    }

    async fn claimed(&self, occurrence: &str) -> Result<bool, OccurrenceLockError> {
        Ok(CLAIMED
            .lock()
            .expect("lock")
            .iter()
            .any(|claimed| claimed == occurrence))
    }
}

/// A binding as a backend writes one: a declared factory for the port, carrying
/// the port's remedy, deduplicated like a `#[module]` expansion.
struct RecordingLockModule;

impl Module for RecordingLockModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
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
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        builder
            .provide_declared_factory::<Arc<dyn OccurrenceLock>, _, _>(BACKEND_REMEDY, |_| async {
                Ok(Arc::new(RecordingLock) as Arc<dyn OccurrenceLock>)
            })
    }
}

#[module(imports = [ScheduleModule, RecordingLockModule, ReplicatedTasksModule])]
struct BoundRoot;

#[module(imports = [ScheduleModule, RecordingLockModule, SecondLockModule, ReplicatedTasksModule])]
struct ContestedRoot;

#[module(imports = [ScheduleModule, ReplicatedTasksModule])]
struct UnboundRoot;

/// The documented wiring runs: the decorator's `replicas = "one"` reaches the
/// scheduler, the binding's lock is the one it claims through, every claim is
/// made under the job's identity and an instant on a multiple of the period, and
/// each occurrence the lock granted fired once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_firing_once_claims_each_occurrence_through_the_bound_lock() {
    let declared = declared();
    assert_eq!(declared.replicas, Replicas::One);

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
    let identity = format!("{}:{}:", declared.provider, declared.method);
    for token in &claimed {
        let instant: u64 = token
            .strip_prefix(&identity)
            .and_then(|instant| instant.parse().ok())
            .unwrap_or_else(|| panic!("a claim is `{identity}<instant>`: {token}"));
        assert_eq!(instant % 100, 0, "ticks on the epoch: {token}");
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
