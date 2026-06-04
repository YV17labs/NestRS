//! End-to-end: a real `#[injectable]` + `#[scheduled]` provider sees its
//! method fire inside a real `App::run`, with the framework auto-attaching
//! the scheduler because the app imports `ScheduleModule`.
//!
//! Pinned to a multi-threaded runtime so the cancellation race against
//! `App::run`'s SIGINT handler does not starve the scheduled tick.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nestrs_core::{injectable, module, App};
use nestrs_schedule::{scheduled, ScheduleModule};
use tokio::time::{sleep, Duration};

static HITS: AtomicUsize = AtomicUsize::new(0);

pub struct Counter(pub AtomicUsize);

#[injectable]
pub struct Tasks {
    #[inject]
    counter: Arc<Counter>,
}

#[scheduled]
impl Tasks {
    #[every("50ms")]
    async fn tick(&self) -> anyhow::Result<()> {
        self.counter.0.fetch_add(1, Ordering::SeqCst);
        HITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// `#[module]` submits the ModuleDescriptor that puts `Tasks` in
// `ReachableProviders` — the scheduler's filter would skip the entry
// otherwise. Counter comes in as a seed (global infrastructure), so the
// access graph accepts the `#[inject]`.
#[module(providers = [Tasks])]
struct TasksModule;

#[module(imports = [TasksModule, ScheduleModule])]
struct AppRoot;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_module_auto_attaches_the_scheduler_and_ticks_the_method() {
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let app = App::builder()
        .provide_arc(counter.clone())
        .module::<AppRoot>()
        .build()
        .await
        .expect("AppRoot builds with ScheduleModule");

    // Drive App::run in the background; signal SIGINT after the tick has had
    // time to fire so the framework's shutdown path drains cleanly — mirrors a
    // real `main`'s lifecycle without any test-only seam in `App::run`.
    let handle = tokio::spawn(app.run());

    sleep(Duration::from_millis(250)).await;
    #[cfg(unix)]
    {
        // SAFETY: raising a signal at the OS level is safe; the framework's
        // signal handler runs in a dedicated task.
        unsafe {
            libc::raise(libc::SIGINT);
        }
    }
    handle
        .await
        .expect("App::run task joins")
        .expect("App::run returns Ok on graceful shutdown");

    let hits = counter.0.load(Ordering::SeqCst);
    assert!(
        hits >= 2,
        "the scheduled method fired at least twice in 250ms (got {hits})",
    );
    // HITS lets a parallel test detect cross-contamination; it's incremented
    // by the same closure, so it must match Tasks' own counter.
    assert_eq!(HITS.load(Ordering::SeqCst), hits);
}
