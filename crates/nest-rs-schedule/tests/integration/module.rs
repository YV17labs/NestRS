//! A `#[scheduled]` provider's method fires inside a real `App::run` once the
//! app imports `ScheduleModule`, and the decorator's grammar compiles.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, injectable, module};
use nest_rs_schedule::{ScheduleModule, scheduled};
use tokio::time::{Duration, sleep};

static HITS: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Counter(pub AtomicUsize);

#[injectable]
pub(crate) struct Tasks {
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

#[module(providers = [Tasks])]
struct TasksModule;

#[module(imports = [TasksModule, ScheduleModule])]
struct AppRoot;

#[tokio::test(start_paused = true)]
async fn schedule_module_auto_attaches_the_scheduler_and_ticks_the_method() {
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let app = App::builder()
        .provide_arc(counter.clone())
        .module::<AppRoot>()
        .build()
        .await
        .expect("AppRoot builds with ScheduleModule");

    let handle = tokio::spawn(app.run());

    sleep(Duration::from_millis(250)).await;
    #[cfg(unix)]
    {
        // SAFETY: raising a signal at the OS level is safe; the framework's
        // signal handler runs in a dedicated task.
        #[expect(
            unsafe_code,
            reason = "only a real SIGINT drives the real shutdown path"
        )]
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
    assert_eq!(HITS.load(Ordering::SeqCst), hits);
}

/// Named so the attribute fits on one line: rustfmt reindents a wrapped
/// attribute inside a `macro_rules!` body further on every run.
const CRON_EXPRESSION: &str = nest_rs_schedule::CronExpression::EVERY_MINUTE;

// `transactional` through a `$settle:expr` fragment, at both positions the
// trigger grammar allows: the substitution arrives as an `Expr::Group`.
macro_rules! declare_scheduled_settlement {
    ($settle:expr) => {
        #[injectable]
        #[derive(Default)]
        pub(crate) struct FragmentTasks;

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

#[injectable]
#[derive(Default)]
struct ShapedTasks;

#[scheduled]
impl ShapedTasks {
    #[expect(
        clippy::needless_arbitrary_self_type,
        reason = "the spelled-out receiver is the shape under test"
    )]
    #[every("30s")]
    async fn typed_receiver(self: &Self) -> anyhow::Result<()> {
        Ok(())
    }

    #[cfg(any())]
    #[every("30s")]
    async fn compiled_out(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s")]
    async fn r#type(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("30s")]
    async fn arc_receiver(self: &std::sync::Arc<Self>) -> anyhow::Result<()> {
        Ok(())
    }
}

#[injectable]
#[derive(Default)]
#[expect(
    non_camel_case_types,
    reason = "a raw-identifier type is the shape under test"
)]
struct r#yield;

#[scheduled]
impl r#yield {
    #[every("30s")]
    async fn sweep(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Compiling is the first half: a trigger compiled out must take its entry with
/// it, or the entry's `run` names a method that does not exist.
#[test]
fn a_compiled_out_trigger_submits_nothing_and_a_typed_receiver_is_scheduled() {
    let mut methods: Vec<&str> =
        nest_rs_core::inventory::iter::<nest_rs_schedule::ScheduledMethod>()
            .filter(|m| (m.provider_type_id)() == std::any::TypeId::of::<ShapedTasks>())
            .map(|m| m.method)
            .collect();
    methods.sort_unstable();
    assert_eq!(
        methods,
        ["arc_receiver", "type", "typed_receiver"],
        "a raw identifier is labelled by its name"
    );
    let raw_hosts: Vec<&str> = nest_rs_core::inventory::iter::<nest_rs_schedule::ScheduledMethod>()
        .filter(|m| (m.provider_type_id)() == std::any::TypeId::of::<r#yield>())
        .map(|m| m.provider)
        .collect();
    assert_eq!(raw_hosts, ["yield"], "a raw host is labelled by its name");
}
