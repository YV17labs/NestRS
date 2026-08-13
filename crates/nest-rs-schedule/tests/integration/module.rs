//! End-to-end: a real `#[injectable]` + `#[scheduled]` provider sees its
//! method fire inside a real `App::run`, with the framework auto-attaching
//! the scheduler because the app imports `ScheduleModule`.
//!
//! Pinned to a multi-threaded runtime so the cancellation race against
//! `App::run`'s SIGINT handler does not starve the scheduled tick.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, injectable, module};
use nest_rs_schedule::{ScheduleModule, scheduled};
use tokio::time::{Duration, sleep};

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

/// The shared `transactional` key read through a `macro_rules!` fragment, at
/// **both** positions the trigger grammar allows.
///
/// A `$settle:expr` substitution arrives as an invisible-delimiter
/// `Expr::Group`, and syn unwraps it only when the parse fork it is read in is
/// empty — so `#[cron(EXPR, transactional = $settle, tz = "UTC")]` was refused
/// while the same key one position later compiled, with a message telling the
/// developer to write the value they had written. Compiling this is the
/// assertion.
/// The cron expression the fragment case uses, named so the attribute fits on
/// one line — rustfmt does not reach inside a `macro_rules!` body and reindents
/// a wrapped attribute a little further on every run.
const CRON_EXPRESSION: &str = nest_rs_schedule::CronExpression::EVERY_MINUTE;

macro_rules! declare_scheduled_settlement {
    ($settle:expr) => {
        #[injectable]
        #[derive(Default)]
        pub struct FragmentTasks;

        #[scheduled]
        impl FragmentTasks {
            #[every("30s", transactional = $settle)]
            async fn last_argument(&self) -> anyhow::Result<()> {
                Ok(())
            }

            #[cron(CRON_EXPRESSION, transactional = $settle, tz = "UTC")]
            async fn not_the_last_argument(&self) -> anyhow::Result<()> {
                Ok(())
            }
        }
    };
}

declare_scheduled_settlement!(false);

#[test]
fn a_transactional_fragment_is_read_the_same_wherever_the_key_sits() {
    let entries = nest_rs_core::inventory::iter::<nest_rs_schedule::ScheduledMethod>()
        .filter(|m| (m.provider_type_id)() == std::any::TypeId::of::<FragmentTasks>())
        .filter(|m| m.transaction == nest_rs_schedule::nest_rs_worker::JobTransaction::Pool)
        .count();
    assert_eq!(
        entries, 2,
        "both fragment-declared triggers registered, and both read the fragment \
         as the `false` it was — not as the `PerAttempt` default",
    );
}
