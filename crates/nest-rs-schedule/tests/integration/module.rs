//! A listed `#[scheduled]` provider's method fires inside a real `App::run`,
//! since the host brings `ScheduleModule`, and the decorator's grammar compiles.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering, injectable, module};
use nest_rs_schedule::{ScheduleModule, scheduled};
use nest_rs_testing::HeadlessApp;
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

#[module(imports = [TasksModule])]
struct AppRoot;

#[tokio::test]
async fn a_scheduled_host_registered_by_hand_with_no_scheduler_is_named_at_run() {
    let logs = nest_rs_testing::LogCapture::install();
    App::builder()
        .provide(Tasks {
            counter: Arc::new(Counter(AtomicUsize::new(0))),
        })
        .build()
        .await
        .expect("the app boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(
        nest_rs_core::target::APP,
        "host not served: its edge's transport is not attached",
    );
    assert_eq!(unserved.field("host").as_deref(), Some("Tasks"));
    assert_eq!(unserved.field("import").as_deref(), Some("ScheduleModule"));
}

#[module(imports = [ScheduleModule])]
struct SchedulerAlone;

/// Following that line serves the seed: the scheduler reads what the app reaches,
/// and a seed is reached.
#[tokio::test(start_paused = true)]
async fn a_scheduled_host_seeded_beside_the_scheduler_ticks_the_method() {
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let logs = nest_rs_testing::LogCapture::install();
    let app = App::builder()
        .provide(Tasks {
            counter: counter.clone(),
        })
        .module::<SchedulerAlone>()
        .build()
        .await
        .expect("the app boots");
    let lifecycle = app
        .container()
        .get::<nest_rs_core::AppLifecycle>()
        .expect("every app is seeded its lifecycle");
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the scheduler runs");
    sleep(Duration::from_millis(250)).await;
    running.abort();

    assert!(counter.0.load(Ordering::SeqCst) >= 2, "the method ticks");
    logs.expect_none(
        nest_rs_core::target::APP,
        "host not served: its edge's transport is not attached",
    );
}

/// Registers the host itself beside the scheduler, which reads what a `#[module]`
/// lists: what the line above says to import is here, and this one takes over.
struct HandRegistersTasksBesideTheScheduler;

impl Module for HandRegistersTasksBesideTheScheduler {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.import::<ScheduleModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<ScheduleModule>().provide(Tasks {
            counter: Arc::new(Counter(AtomicUsize::new(0))),
        })
    }
}

#[tokio::test(start_paused = true)]
async fn a_scheduled_host_registered_by_hand_beside_the_scheduler_is_told_to_list_it() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = App::builder()
        .module::<HandRegistersTasksBesideTheScheduler>()
        .build()
        .await
        .expect("the app boots");
    let lifecycle = app
        .container()
        .get::<nest_rs_core::AppLifecycle>()
        .expect("every app is seeded its lifecycle");
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the scheduler runs");
    running.abort();

    let skipped = logs.find(
        nest_rs_schedule::TARGET,
        "skipped scheduled method: no instance of the provider in this app's container",
    );
    let tasks = skipped
        .iter()
        .find(|line| line.field("host").as_deref() == Some("Tasks"))
        .unwrap_or_else(|| panic!("the host is not said: {skipped:?}"));
    assert_eq!(
        tasks.field("cause").as_deref(),
        Some("registered_outside_modules")
    );
    logs.expect_none(
        nest_rs_core::target::APP,
        "host not served: its edge's transport is not attached",
    );
}

#[tokio::test(start_paused = true)]
async fn a_listed_scheduled_host_brings_the_scheduler_and_ticks_the_method() {
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let app = App::builder()
        .provide_arc(counter.clone())
        .module::<AppRoot>()
        .build()
        .await
        .expect("AppRoot builds");

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

#[injectable]
#[derive(Default)]
struct PurgeTasks;

#[scheduled]
impl PurgeTasks {
    #[every("1h")]
    async fn purge(&self) -> anyhow::Result<()> {
        Ok(())
    }

    #[every("24h")]
    async fn compact(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// An adapter as a developer writes it: the tasks, and nothing of the edge's.
#[module(providers = [PurgeTasks])]
struct PurgeScheduleModule;

/// The app `M` composes, booted with no transport started.
async fn booted<M: Module>() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the module boots")
}

#[tokio::test]
async fn a_listed_scheduled_host_attaches_one_scheduler_for_its_methods() {
    assert_eq!(
        booted::<PurgeScheduleModule>().await.attached_transports(),
        ["schedule"],
    );
}

#[injectable]
#[derive(Default)]
struct DisabledTasks;

#[scheduled]
impl DisabledTasks {
    #[cfg(any())]
    #[every("1h")]
    async fn purge(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(providers = [DisabledTasks])]
struct DisabledScheduleModule;

#[tokio::test]
async fn a_scheduled_host_whose_methods_are_compiled_out_brings_no_scheduler() {
    assert!(
        booted::<DisabledScheduleModule>()
            .await
            .attached_transports()
            .is_empty()
    );
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
        .filter(|m| m.provider_type_id() == std::any::TypeId::of::<FragmentTasks>())
        .filter(|m| m.transaction() == nest_rs_database::TransactionPolicy::Pool)
        .count();
    assert_eq!(
        entries, 2,
        "both fragment-declared triggers registered, and both read the fragment \
         as the `false` it was — not as the `PerUnit` default",
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
            .filter(|m| m.provider_type_id() == std::any::TypeId::of::<ShapedTasks>())
            .map(|m| m.method())
            .collect();
    methods.sort_unstable();
    assert_eq!(
        methods,
        ["arc_receiver", "type", "typed_receiver"],
        "a raw identifier is labelled by its name"
    );
    let raw_hosts: Vec<&str> = nest_rs_core::inventory::iter::<nest_rs_schedule::ScheduledMethod>()
        .filter(|m| m.provider_type_id() == std::any::TypeId::of::<r#yield>())
        .map(|m| m.provider())
        .collect();
    assert_eq!(raw_hosts, ["yield"], "a raw host is labelled by its name");
}
