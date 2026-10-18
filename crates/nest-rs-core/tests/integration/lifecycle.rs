//! Covers `src/lifecycle.rs` — hook shapes, a host bound as `dyn Trait`, the
//! order hosts run in, the shutdown phases after every transport stopped or
//! was abandoned, shutdown hooks that fail, hang or panic, and each hook run as
//! one `lifecycle.hook` unit of work.

use nest_rs_core::{operation_log, target};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nest_rs_core::__private::{HookFuture, LifecycleUnitOfWork};
use nest_rs_core::{
    App, Container, ContainerBuilder, LifecyclePhase, Module, Registering, SHUTDOWN_HOOKS_TIMEOUT,
    Transport, hooks, injectable, module,
};
use nest_rs_testing::{CapturedEvent, LogCapture};
use tokio_util::sync::CancellationToken;

use crate::Failing;

/// Every `lifecycle.hook` line `logs` holds.
fn hook_lines(logs: &LogCapture) -> Vec<CapturedEvent> {
    logs.find(operation_log::TARGET, nest_rs_core::unit::HOOK.name())
}

/// The `lifecycle.hook` line of the hook named `method`, filed once.
fn hook_line(logs: &LogCapture, method: &str) -> CapturedEvent {
    let mut lines: Vec<_> = hook_lines(logs)
        .into_iter()
        .filter(|line| line.field("method").as_deref() == Some(method))
        .collect();
    assert_eq!(lines.len(), 1, "one line for `{method}`, got {lines:#?}");
    lines.remove(0)
}

trait Bridge: Send + Sync {}

static DYN_ONLY: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct DynOnlyHost;

impl Bridge for DynOnlyHost {}

#[hooks]
impl DynOnlyHost {
    #[on_module_init]
    async fn init(&self) {
        DYN_ONLY.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [DynOnlyHost as dyn Bridge])]
struct DynOnlyModule;

#[tokio::test]
async fn a_host_bound_as_dyn_fires_its_hooks_once() {
    let app = App::new::<DynOnlyModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");

    assert_eq!(
        DYN_ONLY.load(Ordering::SeqCst),
        1,
        "the container holds the host under its own type beside `Arc<dyn Bridge>`",
    );
}

#[injectable]
#[derive(Default)]
struct BrokenOnDestroy;

#[hooks]
impl BrokenOnDestroy {
    #[on_module_destroy]
    async fn release(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("the migration lock was already held"))
    }
}

#[module(providers = [BrokenOnDestroy])]
struct BrokenOnDestroyModule;

#[tokio::test]
async fn a_failing_shutdown_hook_is_named_at_error_and_does_not_abort_the_rest() {
    let logs = LogCapture::install();
    // No transports: `run` goes straight to the shutdown phases.
    App::new::<BrokenOnDestroyModule>()
        .expect("the module boots")
        .run()
        .await
        .expect("a failed cleanup is swallowed: shutdown is best-effort");

    let event = logs.expect_one(target::LIFECYCLE, "lifecycle hook failed");
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("provider")
            .is_some_and(|p| p.contains("BrokenOnDestroy")),
        "{:?}",
        event.fields,
    );
    assert_eq!(event.field("method").as_deref(), Some("release"));
    assert_eq!(event.field("phase").as_deref(), Some("OnModuleDestroy"));
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("migration lock")),
        "the event carries the hook's own error, got {:?}",
        event.fields,
    );
}

static SHAPED_INITS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct ShapedHost;

#[hooks]
impl ShapedHost {
    #[expect(
        clippy::needless_arbitrary_self_type,
        reason = "the spelled-out receiver is the shape under test"
    )]
    #[on_module_init]
    async fn init(self: &Self) -> () {
        SHAPED_INITS.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(any())]
    #[on_module_init]
    async fn compiled_out(&self) {}

    #[on_module_init]
    async fn r#type(&self) {
        SHAPED_INITS.fetch_add(1, Ordering::SeqCst);
    }

    #[on_module_init]
    async fn through_its_arc(self: &std::sync::Arc<Self>) {
        SHAPED_INITS.fetch_add(1, Ordering::SeqCst);
    }
}

#[injectable]
#[derive(Default)]
#[expect(
    non_camel_case_types,
    reason = "a raw-identifier type is the shape under test"
)]
struct r#async;

#[hooks]
impl r#async {
    #[on_module_init]
    async fn init(&self) {}
}

#[module(providers = [ShapedHost])]
struct ShapedModule;

#[tokio::test]
async fn a_compiled_out_hook_is_skipped_and_the_spelled_out_shapes_run() {
    let mut methods: Vec<&str> =
        nest_rs_core::inventory::iter::<nest_rs_core::__private::LifecycleHook>()
            .filter(|hook| hook.provider == "ShapedHost")
            .map(|hook| hook.method)
            .collect();
    methods.sort_unstable();
    assert_eq!(methods, ["init", "through_its_arc", "type"]);
    let raw_hosts = nest_rs_core::inventory::iter::<nest_rs_core::__private::LifecycleHook>()
        .filter(|hook| hook.provider == "async")
        .count();
    assert_eq!(raw_hosts, 1, "a raw host is labelled by its name");

    let app = App::new::<ShapedModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");
    assert_eq!(SHAPED_INITS.load(Ordering::SeqCst), 3);
}

#[expect(
    dead_code,
    reason = "the trait exists only to put a hook's name on Arc<T>"
)]
trait Warm {
    fn warm(&self) -> std::future::Ready<()>;
}

impl<T> Warm for std::sync::Arc<T> {
    fn warm(&self) -> std::future::Ready<()> {
        std::future::ready(())
    }
}

static WARMED: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct WarmHost;

#[hooks]
impl WarmHost {
    #[on_module_init]
    async fn warm(&self) {
        WARMED.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [WarmHost])]
struct WarmModule;

/// Method-call syntax on the `Arc<Host>` finds a trait method on `Arc<T>` first.
#[tokio::test]
async fn a_hook_runs_where_a_trait_on_arc_shares_its_name() {
    let app = App::new::<WarmModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");
    assert_eq!(WARMED.load(Ordering::SeqCst), 1);
}

const ABANDONED: &str = "shutdown hook abandoned: the shutdown hooks' budget was spent while it \
                         waited, and the hooks after it still start";

static TIDIED: AtomicUsize = AtomicUsize::new(0);

/// Injects `TidyOnDestroy`, so its shutdown hook runs first.
#[injectable]
struct StuckOnDestroy {
    #[inject]
    _tidy: std::sync::Arc<TidyOnDestroy>,
}

#[hooks]
impl StuckOnDestroy {
    #[on_module_destroy]
    async fn flush(&self) {
        std::future::pending::<()>().await;
    }
}

/// Runs after `StuckOnDestroy` in their shared phase, and has a hook in each
/// phase after it.
#[injectable]
#[derive(Default)]
struct TidyOnDestroy;

#[hooks]
impl TidyOnDestroy {
    #[on_module_destroy]
    async fn release(&self) {
        TIDIED.fetch_add(1, Ordering::SeqCst);
    }

    #[before_application_shutdown]
    async fn announce(&self) {
        TIDIED.fetch_add(1, Ordering::SeqCst);
    }

    #[on_application_shutdown]
    async fn close(&self) {
        TIDIED.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [StuckOnDestroy, TidyOnDestroy])]
struct StuckOnDestroyModule;

#[tokio::test(start_paused = true)]
async fn a_shutdown_hook_that_never_returns_is_abandoned_at_the_budget_and_the_rest_still_run() {
    let logs = LogCapture::install();
    let started = tokio::time::Instant::now();
    App::new::<StuckOnDestroyModule>()
        .expect("the module boots")
        .run()
        .await
        .expect("an abandoned cleanup is not an error: shutdown is best-effort");
    let took = started.elapsed();

    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "shutdown waited on the stuck hook for the budget and no longer, took {took:?}",
    );
    assert_eq!(
        TIDIED.load(Ordering::SeqCst),
        3,
        "the hook after it in its phase, and one in each later phase, all ran",
    );
    let event = logs.expect_one(target::LIFECYCLE, ABANDONED);
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("provider")
            .is_some_and(|p| p.contains("StuckOnDestroy")),
        "{:?}",
        event.fields,
    );
    assert_eq!(event.field("method").as_deref(), Some("flush"));
    assert_eq!(event.field("phase").as_deref(), Some("OnModuleDestroy"));
    assert!(
        event
            .field("origin")
            .is_some_and(|origin| origin.ends_with("::lifecycle")),
        "the line names the module the hook lives in, got {:?}",
        event.fields,
    );
    assert_eq!(
        event.field("waited_ms"),
        Some(SHUTDOWN_HOOKS_TIMEOUT.as_millis().to_string()),
    );
    assert_eq!(
        event.field("budget_ms"),
        Some(SHUTDOWN_HOOKS_TIMEOUT.as_millis().to_string()),
    );
    let flush = hook_line(&logs, "flush");
    assert_eq!(
        flush.field("outcome").as_deref(),
        Some(operation_log::CANCELLED),
        "the hook abandoned at the budget files its unit cancelled",
    );
    assert_eq!(
        event.trace_id, flush.trace_id,
        "the abandonment is said in the hook's own trace",
    );
    for method in ["release", "announce", "close"] {
        assert_eq!(
            hook_line(&logs, method).field("outcome").as_deref(),
            Some(operation_log::OK),
            "{method}",
        );
    }
}

#[injectable]
#[derive(Default)]
struct StuckEverywhere;

#[hooks]
impl StuckEverywhere {
    #[on_module_destroy]
    async fn a(&self) {
        std::future::pending::<()>().await;
    }

    #[on_module_destroy]
    async fn b(&self) {
        std::future::pending::<()>().await;
    }

    #[before_application_shutdown]
    async fn c(&self) {
        std::future::pending::<()>().await;
    }

    #[on_application_shutdown]
    async fn d(&self) {
        std::future::pending::<()>().await;
    }
}

#[module(providers = [StuckEverywhere])]
struct StuckEverywhereModule;

#[tokio::test(start_paused = true)]
async fn four_stuck_shutdown_hooks_share_one_budget_and_each_is_named() {
    let logs = LogCapture::install();
    let started = tokio::time::Instant::now();
    App::new::<StuckEverywhereModule>()
        .expect("the module boots")
        .run()
        .await
        .expect("abandoned cleanups are not an error: shutdown is best-effort");
    let took = started.elapsed();

    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "the four stuck hooks were held to one budget between them, took {took:?}",
    );
    let abandoned: Vec<_> = logs
        .find(target::LIFECYCLE, ABANDONED)
        .into_iter()
        .filter(|event| {
            event
                .field("provider")
                .is_some_and(|p| p.contains("StuckEverywhere"))
        })
        .map(|event| {
            (
                event.field("method").unwrap_or_default(),
                event.field("waited_ms").unwrap_or_default(),
            )
        })
        .collect();
    let budget = SHUTDOWN_HOOKS_TIMEOUT.as_millis().to_string();
    assert_eq!(
        abandoned,
        vec![
            ("a".to_owned(), budget),
            ("b".to_owned(), "0".to_owned()),
            ("c".to_owned(), "0".to_owned()),
            ("d".to_owned(), "0".to_owned()),
        ],
        "the first spends the budget, and each one after it is started, abandoned and named",
    );
}

static AFTER_PANIC: AtomicUsize = AtomicUsize::new(0);

/// Injects `AfterPanic`, so its shutdown hook runs first.
#[injectable]
struct PanicsOnDestroy {
    #[inject]
    _after: std::sync::Arc<AfterPanic>,
}

#[hooks]
impl PanicsOnDestroy {
    #[on_module_destroy]
    async fn release(&self) {
        panic!("the pool was already closed");
    }
}

/// Runs after `PanicsOnDestroy` in its phase, and has a hook in a later one.
#[injectable]
#[derive(Default)]
struct AfterPanic;

#[hooks]
impl AfterPanic {
    #[on_module_destroy]
    async fn release(&self) {
        AFTER_PANIC.fetch_add(1, Ordering::SeqCst);
    }

    #[on_application_shutdown]
    async fn close(&self) {
        AFTER_PANIC.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [AfterPanic, PanicsOnDestroy])]
struct PanicOnDestroyModule;

#[tokio::test]
async fn a_panicking_shutdown_hook_is_named_at_error_and_the_rest_still_run() {
    let logs = LogCapture::install();
    let outcome = tokio::spawn(async {
        App::new::<PanicOnDestroyModule>()
            .expect("the module boots")
            .run()
            .await
    })
    .await;

    assert!(
        matches!(outcome, Ok(Ok(()))),
        "a panicking cleanup is contained: shutdown is best-effort, got {outcome:?}",
    );
    assert_eq!(
        AFTER_PANIC.load(Ordering::SeqCst),
        2,
        "the hook after it in its phase, and the one in a later phase, both ran",
    );
    let event = logs.expect_one(
        target::LIFECYCLE,
        "shutdown hook panicked, and the hooks after it still run",
    );
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("provider")
            .is_some_and(|p| p.contains("PanicsOnDestroy")),
        "{:?}",
        event.fields,
    );
    assert_eq!(event.field("method").as_deref(), Some("release"));
    assert_eq!(event.field("phase").as_deref(), Some("OnModuleDestroy"));
    assert_eq!(
        event.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the pool was already closed"),
    );
}

#[injectable]
#[derive(Default)]
struct PanicsOnInit;

#[hooks]
impl PanicsOnInit {
    #[on_module_init]
    async fn warm(&self) {
        panic!("the cache backend refused the warm-up");
    }
}

#[module(providers = [PanicsOnInit])]
struct PanicOnInitModule;

#[tokio::test]
async fn a_panicking_init_hook_aborts_the_boot_with_an_error_naming_it() {
    let logs = LogCapture::install();
    let outcome = tokio::spawn(async {
        App::new::<PanicOnInitModule>()
            .expect("the module boots")
            .run()
            .await
    })
    .await
    .expect("the panic is contained, not propagated out of `App::run`");
    let error = outcome.expect_err("an init hook that panicked aborts the boot");
    assert_eq!(
        error.to_string(),
        "lifecycle hook integration::lifecycle::PanicsOnInit::warm (OnModuleInit) panicked",
    );
    let event = logs.expect_one(
        target::LIFECYCLE,
        "lifecycle hook panicked; the boot is aborted",
    );
    assert_eq!(event.level, "error");
    assert_eq!(event.field("method").as_deref(), Some("warm"));
    assert_eq!(event.field("phase").as_deref(), Some("OnModuleInit"));
    assert_eq!(
        event.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the cache backend refused the warm-up"),
    );
}

static ONCE_INITS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct CountsInit;

#[hooks]
impl CountsInit {
    #[on_module_init]
    async fn count(&self) {
        ONCE_INITS.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [CountsInit])]
struct CountsInitModule;

#[tokio::test]
async fn an_app_runs_its_init_hooks_once_however_often_it_is_initialised() {
    let app = App::new::<CountsInitModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");
    app.init().await.expect("a second init is a no-op");
    app.run().await.expect("the run serves nothing and stops");

    assert_eq!(ONCE_INITS.load(Ordering::SeqCst), 1);
}

static FAILED_INITS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct FailsInit;

#[hooks]
impl FailsInit {
    #[on_module_init]
    async fn connect(&self) -> anyhow::Result<()> {
        FAILED_INITS.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!(
            "the discovery backend refused the registration"
        ))
    }
}

#[module(providers = [FailsInit])]
struct FailsInitModule;

#[tokio::test]
async fn an_init_that_failed_is_refused_again_without_a_hook_running() {
    let app = App::new::<FailsInitModule>().expect("the module boots");
    let first = app.init().await.expect_err("the hook fails the init");
    assert!(
        format!("{first:#}").contains("discovery backend"),
        "{first:#}"
    );

    let again = app
        .init()
        .await
        .expect_err("a failed init does not run again");
    assert!(
        again
            .downcast_ref::<nest_rs_core::InitFailedError>()
            .is_some(),
        "{again:#}"
    );
    let run = app
        .run()
        .await
        .expect_err("nor does the run that follows it");
    assert!(
        run.downcast_ref::<nest_rs_core::InitFailedError>()
            .is_some(),
        "{run:#}"
    );
    assert_eq!(FAILED_INITS.load(Ordering::SeqCst), 1);
}

static WARMED_BEFORE_THE_DROP: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct WarmsThenWaits;

#[hooks]
impl WarmsThenWaits {
    #[on_module_init]
    async fn warm(&self) {
        WARMED_BEFORE_THE_DROP.fetch_add(1, Ordering::SeqCst);
    }

    #[on_application_bootstrap]
    async fn announce(&self) {
        std::future::pending::<()>().await;
    }
}

#[module(providers = [WarmsThenWaits])]
struct WarmsThenWaitsModule;

#[tokio::test(start_paused = true)]
async fn an_init_dropped_while_a_hook_waits_is_refused_again_without_a_hook_running() {
    let logs = LogCapture::install();
    let app = App::new::<WarmsThenWaitsModule>().expect("the module boots");
    tokio::time::timeout(Duration::from_secs(1), app.init())
        .await
        .expect_err("the init waits on its second hook until the caller drops it");

    let again = tokio::time::timeout(Duration::from_secs(1), app.init())
        .await
        .expect("the second init runs no hook, so none holds it")
        .expect_err("an init that never finished does not run again");
    assert!(
        again
            .downcast_ref::<nest_rs_core::InitFailedError>()
            .is_some(),
        "{again:#}"
    );
    assert_eq!(
        WARMED_BEFORE_THE_DROP.load(Ordering::SeqCst),
        1,
        "the hook that finished ran once"
    );
    assert_eq!(
        hook_line(&logs, "warm").field("outcome").as_deref(),
        Some(operation_log::OK),
    );
    assert_eq!(
        hook_line(&logs, "announce").field("outcome").as_deref(),
        Some(operation_log::CANCELLED),
        "the hook dropped with the init files its unit cancelled",
    );
}

static CACHES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

mod first {
    use nest_rs_core::{hooks, injectable, module};

    #[injectable]
    #[derive(Default)]
    pub(crate) struct Cache;

    #[hooks]
    impl Cache {
        #[on_module_init]
        async fn warm(&self) {
            super::CACHES
                .lock()
                .expect("log mutex is not poisoned")
                .push(module_path!());
        }
    }

    #[module(providers = [Cache])]
    pub(crate) struct FirstCacheModule;
}

mod second {
    use nest_rs_core::{hooks, injectable, module};

    #[injectable]
    #[derive(Default)]
    pub(crate) struct Cache;

    #[hooks]
    impl Cache {
        #[on_module_init]
        async fn warm(&self) {
            super::CACHES
                .lock()
                .expect("log mutex is not poisoned")
                .push(module_path!());
        }
    }

    #[module(providers = [Cache])]
    pub(crate) struct SecondCacheModule;
}

#[module(imports = [first::FirstCacheModule, second::SecondCacheModule])]
struct FirstThenSecondModule;

#[module(imports = [second::SecondCacheModule, first::FirstCacheModule])]
struct SecondThenFirstModule;

/// The order each build of the root ran its two `Cache` hosts in.
async fn caches_warmed<M: nest_rs_core::Module + 'static>() -> Vec<&'static str> {
    let mut builds = Vec::new();
    for _ in 0..2 {
        CACHES.lock().expect("log mutex is not poisoned").clear();
        App::new::<M>()
            .expect("the module boots")
            .init()
            .await
            .expect("the init phases drain");
        let warmed = CACHES.lock().expect("log mutex is not poisoned").clone();
        builds.push(warmed);
    }
    assert_eq!(builds[0], builds[1], "two builds of one root ran one order");
    builds.remove(0)
}

#[tokio::test]
async fn two_hosts_of_one_name_run_in_their_modules_import_order_in_every_build() {
    assert_eq!(
        caches_warmed::<FirstThenSecondModule>().await,
        [
            "integration::lifecycle::first",
            "integration::lifecycle::second"
        ],
    );
    assert_eq!(
        caches_warmed::<SecondThenFirstModule>().await,
        [
            "integration::lifecycle::second",
            "integration::lifecycle::first"
        ],
    );
}

#[injectable]
#[derive(Default)]
struct RegistersThenHangs;

#[hooks]
impl RegistersThenHangs {
    #[on_module_init]
    async fn register(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("the discovery backend refused the lease"))
    }

    #[on_module_destroy]
    async fn deregister(&self) {
        std::future::pending::<()>().await;
    }
}

#[module(providers = [RegistersThenHangs])]
struct RegistersThenHangsModule;

#[tokio::test(start_paused = true)]
async fn a_failed_boot_tears_down_within_the_budget_and_returns_the_init_error() {
    let logs = LogCapture::install();
    let started = tokio::time::Instant::now();
    let error = App::new::<RegistersThenHangsModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("the init hook failed the boot");
    let took = started.elapsed();

    assert!(
        format!("{error:#}").contains("the discovery backend refused the lease"),
        "the boot error, not the teardown's: {error:#}"
    );
    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "the teardown waited on the hung hook for the budget and no longer, took {took:?}",
    );
    let opening = logs.expect_one(
        target::LIFECYCLE,
        "the boot failed after init hooks ran: running the shutdown hooks",
    );
    assert_eq!(opening.level, "warn");
    assert_eq!(opening.field("phase").as_deref(), Some("OnModuleInit"));
    assert_eq!(
        opening.field("provider").as_deref(),
        Some("RegistersThenHangs")
    );
    assert_eq!(opening.field("method").as_deref(), Some("register"));
    assert!(
        opening
            .field("origin")
            .is_some_and(|origin| origin.ends_with("::lifecycle")),
        "{:?}",
        opening.fields,
    );
    let abandoned = logs.expect_one(target::LIFECYCLE, ABANDONED);
    assert_eq!(abandoned.field("method").as_deref(), Some("deregister"));
}

/// How long [`UnwindsAfterItsStop`] takes to return once its stop fires.
const UNWIND: Duration = Duration::from_secs(2);

/// When [`UnwindsAfterItsStop`]'s `serve` returned.
static UNWOUND_AT: Mutex<Option<tokio::time::Instant>> = Mutex::new(None);

/// Each shutdown hook's phase, with what it read of [`UNWOUND_AT`] as it started.
static HOOKS_SAW: Mutex<Vec<(LifecyclePhase, Option<tokio::time::Instant>)>> =
    Mutex::new(Vec::new());

fn unwound_at() -> Option<tokio::time::Instant> {
    *UNWOUND_AT.lock().unwrap_or_else(PoisonError::into_inner)
}

fn saw(phase: LifecyclePhase) {
    let unwound = unwound_at();
    HOOKS_SAW
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((phase, unwound));
}

/// Serves until its stop, then takes [`UNWIND`] to return, as a transport
/// finishing what it still runs does.
struct UnwindsAfterItsStop;

#[async_trait::async_trait]
impl Transport for UnwindsAfterItsStop {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, stop: CancellationToken) -> anyhow::Result<()> {
        stop.cancelled().await;
        tokio::time::sleep(UNWIND).await;
        *UNWOUND_AT.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(tokio::time::Instant::now());
        Ok(())
    }

    fn stop_bound(&self) -> Duration {
        UNWIND * 2
    }
}

struct UnwindsBesideFailingModule;

impl Module for UnwindsBesideFailingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(UnwindsAfterItsStop))
            .provide_transport(|_| Ok(Failing))
    }
}

#[injectable]
#[derive(Default)]
struct TearsDownAfterServing;

#[hooks]
impl TearsDownAfterServing {
    #[on_module_destroy]
    async fn destroy(&self) {
        saw(LifecyclePhase::OnModuleDestroy);
    }

    #[before_application_shutdown]
    async fn before(&self) {
        saw(LifecyclePhase::BeforeApplicationShutdown);
    }

    #[on_application_shutdown]
    async fn shutdown(&self) {
        saw(LifecyclePhase::OnApplicationShutdown);
    }
}

#[module(imports = [UnwindsBesideFailingModule], providers = [TearsDownAfterServing])]
struct TearsDownAfterServingModule;

/// The three shutdown phases start once every transport's `serve` returned,
/// the slowest included, never at the stop: no hook tears down what a unit
/// still serving uses.
#[tokio::test(start_paused = true)]
async fn no_shutdown_hook_starts_before_every_transport_has_returned_from_serve() {
    App::new::<TearsDownAfterServingModule>()
        .expect("the module boots")
        .run()
        .await
        .expect_err("the failing transport stopped the app");

    let saw = HOOKS_SAW
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        saw.iter().all(|(_, unwound)| unwound.is_some()),
        "a shutdown hook started before every transport returned from `serve`: {saw:?}",
    );
    let unwound = unwound_at().expect("the unwinding transport returned once its stop fired");
    assert_eq!(
        saw,
        [
            (LifecyclePhase::OnModuleDestroy, Some(unwound)),
            (LifecyclePhase::BeforeApplicationShutdown, Some(unwound)),
            (LifecyclePhase::OnApplicationShutdown, Some(unwound)),
        ],
        "each shutdown phase ran once, in order",
    );
}

/// The bound [`IgnoresItsStop`] states.
const IGNORED_BOUND: Duration = Duration::from_secs(3);

/// Each shutdown hook's phase, with the instant it started.
static STARTED_PAST_THE_BOUND: Mutex<Vec<(LifecyclePhase, tokio::time::Instant)>> =
    Mutex::new(Vec::new());

fn started(phase: LifecyclePhase) {
    STARTED_PAST_THE_BOUND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((phase, tokio::time::Instant::now()));
}

/// Never returns from `serve`, its stop ignored.
struct IgnoresItsStop;

#[async_trait::async_trait]
impl Transport for IgnoresItsStop {
    async fn configure(&mut self, _container: &Container) -> anyhow::Result<()> {
        Ok(())
    }

    async fn serve(self: Box<Self>, _stop: CancellationToken) -> anyhow::Result<()> {
        std::future::pending().await
    }

    fn stop_bound(&self) -> Duration {
        IGNORED_BOUND
    }
}

struct IgnoresBesideFailingModule;

impl Module for IgnoresBesideFailingModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide_transport(|_| Ok(IgnoresItsStop))
            .provide_transport(|_| Ok(Failing))
    }
}

#[injectable]
#[derive(Default)]
struct TearsDownPastTheBound;

#[hooks]
impl TearsDownPastTheBound {
    #[on_module_destroy]
    async fn destroy(&self) {
        started(LifecyclePhase::OnModuleDestroy);
    }

    #[before_application_shutdown]
    async fn before(&self) {
        started(LifecyclePhase::BeforeApplicationShutdown);
    }

    #[on_application_shutdown]
    async fn shutdown(&self) {
        started(LifecyclePhase::OnApplicationShutdown);
    }
}

#[module(imports = [IgnoresBesideFailingModule], providers = [TearsDownPastTheBound])]
struct TearsDownPastTheBoundModule;

/// A transport still serving past its bound and the settle is abandoned there,
/// and the three shutdown phases start then, never before it.
#[tokio::test(start_paused = true)]
async fn the_shutdown_phases_start_once_a_transport_ignoring_its_stop_is_abandoned() {
    let app = App::new::<TearsDownPastTheBoundModule>().expect("the module boots");
    let stopped = tokio::time::Instant::now();

    tokio::time::timeout(Duration::from_secs(60), app.run())
        .await
        .expect("the kernel does not wait on the transport past its bound")
        .expect_err("the failing transport stopped the app");

    let abandoned = stopped + IGNORED_BOUND + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT;
    let started = STARTED_PAST_THE_BOUND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        started.iter().all(|(_, at)| *at >= abandoned),
        "a shutdown hook started before the transport was abandoned at {abandoned:?}: {started:?}",
    );
    assert_eq!(
        started.iter().map(|(phase, _)| *phase).collect::<Vec<_>>(),
        [
            LifecyclePhase::OnModuleDestroy,
            LifecyclePhase::BeforeApplicationShutdown,
            LifecyclePhase::OnApplicationShutdown,
        ],
        "each shutdown phase ran once, in order, past the abandonment",
    );
}

#[injectable]
#[derive(Default)]
struct Ledger;

#[hooks]
impl Ledger {
    #[on_module_destroy]
    async fn seal(&self) {}
}

#[injectable]
struct Payouts {
    #[inject]
    _ledger: std::sync::Arc<Ledger>,
}

#[hooks]
impl Payouts {
    #[on_module_destroy]
    async fn settle(&self) {}
}

#[module(providers = [Ledger, Payouts])]
struct PayoutsModule;

#[tokio::test]
async fn each_shutdown_hook_files_its_unit_line_in_reverse_host_order() {
    let logs = LogCapture::install();
    let app = App::new::<PayoutsModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");

    app.close().await;

    let ran: Vec<_> = hook_lines(&logs)
        .into_iter()
        .filter(|event| event.field("phase").as_deref() == Some("OnModuleDestroy"))
        .map(|event| {
            assert_eq!(event.level, "info");
            assert!(
                event
                    .field("origin")
                    .is_some_and(|origin| origin.ends_with("::lifecycle")),
                "{:?}",
                event.fields,
            );
            (event.field("provider"), event.field("method"))
        })
        .collect();
    assert_eq!(
        ran,
        [
            (Some("Payouts".to_owned()), Some("settle".to_owned())),
            (Some("Ledger".to_owned()), Some("seal".to_owned())),
        ],
        "the dependent's hook first, as an operator reads the way down",
    );
}

#[injectable]
#[derive(Default)]
struct Audited;

#[hooks]
impl Audited {
    #[on_module_init]
    async fn open(&self) {}

    #[on_module_destroy]
    async fn reconcile(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("the ledger refused the reconciliation"))
    }

    #[on_application_shutdown]
    async fn report(&self) {
        panic!("the report writer was already gone");
    }
}

#[module(providers = [Audited])]
struct AuditedModule;

#[tokio::test]
async fn each_hook_files_one_unit_line_naming_it_and_how_it_ended() {
    let logs = LogCapture::install();
    let app = App::new::<AuditedModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");
    app.close().await;

    for (method, phase, outcome) in [
        ("open", "OnModuleInit", operation_log::OK),
        ("reconcile", "OnModuleDestroy", operation_log::ERROR),
        ("report", "OnApplicationShutdown", operation_log::PANIC),
    ] {
        let line = hook_line(&logs, method);
        assert_eq!(line.level, "info", "{method}");
        assert_eq!(line.field("outcome").as_deref(), Some(outcome), "{method}");
        assert_eq!(line.field("phase").as_deref(), Some(phase), "{method}");
        assert_eq!(
            line.field("provider").as_deref(),
            Some("Audited"),
            "{method}"
        );
        assert!(
            line.field("origin")
                .is_some_and(|origin| origin.ends_with("::lifecycle")),
            "{method}: {:?}",
            line.fields,
        );
        assert!(line.field(operation_log::DURATION_MS).is_some(), "{method}");
        assert!(
            line.trace_id.is_some() && line.span_id.is_some(),
            "{method}: a hook's line carries the trace minted for it",
        );
        assert!(
            line.actor_id.is_none(),
            "{method}: a hook has no caller, so no actor"
        );
    }
    let traces: std::collections::BTreeSet<_> = ["open", "reconcile", "report"]
        .into_iter()
        .map(|method| hook_line(&logs, method).trace_id)
        .collect();
    assert_eq!(traces.len(), 3, "each hook is a trace of its own");

    let failed = logs.expect_one(target::LIFECYCLE, "lifecycle hook failed");
    assert_eq!(
        failed.trace_id,
        hook_line(&logs, "reconcile").trace_id,
        "the failure's detail is said in the hook's trace",
    );
    let reconcile = hook_line(&logs, "reconcile");
    let spans: Vec<_> = logs
        .spans()
        .into_iter()
        .filter(|span| {
            span.target == target::LIFECYCLE
                && span.name == nest_rs_core::unit::HOOK.name()
                && span.field("method").as_deref() == Some("reconcile")
        })
        .collect();
    assert_eq!(spans.len(), 1, "one span for the hook: {spans:#?}");
    assert_eq!(spans[0].field("otel.kind").as_deref(), Some("internal"));
    assert_eq!(spans[0].field("provider").as_deref(), Some("Audited"));
    assert_eq!(
        spans[0].field("span_id"),
        reconcile.span_id,
        "the line names the span it closes",
    );
    assert!(
        logs.find(target::LIFECYCLE, "running lifecycle hook")
            .is_empty(),
        "the unit line replaces the hook's debug line",
    );
}

tokio::task_local! {
    /// What the hook running now was handed by the bound unit of work: the
    /// deadline it opened its unit with.
    static IN_UNIT: Option<tokio::time::Instant>;
}

/// A unit of work that runs each hook with the deadline it was given ambient.
struct UnitsOnRecord;

impl LifecycleUnitOfWork for UnitsOnRecord {
    fn run<'a>(
        &'a self,
        deadline: Option<tokio::time::Instant>,
        hook: HookFuture<'a>,
    ) -> HookFuture<'a> {
        Box::pin(IN_UNIT.scope(deadline, hook))
    }
}

/// The deadline each hook saw its unit opened with, by phase; `None` for a
/// hook run outside any unit.
static SEEN_IN_UNIT: std::sync::Mutex<Vec<(LifecyclePhase, Option<Option<tokio::time::Instant>>)>> =
    std::sync::Mutex::new(Vec::new());

fn record_unit(phase: LifecyclePhase) {
    SEEN_IN_UNIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((phase, IN_UNIT.try_with(|deadline| *deadline).ok()));
}

fn seen_in_unit() -> Vec<(LifecyclePhase, Option<Option<tokio::time::Instant>>)> {
    SEEN_IN_UNIT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[injectable]
#[derive(Default)]
struct UnitWitness;

#[hooks]
impl UnitWitness {
    #[on_module_init]
    async fn init(&self) {
        record_unit(LifecyclePhase::OnModuleInit);
    }

    #[on_application_bootstrap]
    async fn bootstrap(&self) {
        record_unit(LifecyclePhase::OnApplicationBootstrap);
    }

    #[on_module_destroy]
    async fn destroy(&self) {
        record_unit(LifecyclePhase::OnModuleDestroy);
    }

    #[on_application_shutdown]
    async fn shutdown(&self) {
        record_unit(LifecyclePhase::OnApplicationShutdown);
    }
}

#[module(providers = [UnitWitness])]
struct UnitWitnessModule;

#[tokio::test(start_paused = true)]
async fn every_hook_runs_inside_the_bound_unit_of_work_the_shutdown_ones_to_the_budget() {
    let app = App::builder()
        .provide_dyn::<dyn LifecycleUnitOfWork>(Arc::new(UnitsOnRecord))
        .module::<UnitWitnessModule>()
        .build()
        .await
        .expect("the module boots");
    app.init().await.expect("the init phases drain");
    let closing = tokio::time::Instant::now();
    app.close().await;

    let seen = seen_in_unit();
    let phases: Vec<_> = seen.iter().map(|(phase, _)| *phase).collect();
    assert_eq!(
        phases,
        [
            LifecyclePhase::OnModuleInit,
            LifecyclePhase::OnApplicationBootstrap,
            LifecyclePhase::OnModuleDestroy,
            LifecyclePhase::OnApplicationShutdown,
        ],
    );
    for (phase, deadline) in &seen[..2] {
        assert_eq!(
            *deadline,
            Some(None),
            "{phase:?}: in a unit, cut by nothing"
        );
    }
    for (phase, deadline) in &seen[2..] {
        let deadline = deadline
            .unwrap_or_else(|| panic!("{phase:?} ran outside the unit of work"))
            .unwrap_or_else(|| panic!("{phase:?}'s unit carries no deadline"));
        assert_eq!(
            deadline,
            closing + SHUTDOWN_HOOKS_TIMEOUT,
            "{phase:?}: the shutdown hooks' one budget"
        );
    }
}

#[injectable]
#[derive(Default)]
struct BareWitness;

#[hooks]
impl BareWitness {
    #[on_module_init]
    async fn init(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            IN_UNIT.try_with(|_| ()).is_err(),
            "the hook ran inside a unit of work no app bound",
        );
        Ok(())
    }
}

#[module(providers = [BareWitness])]
struct BareWitnessModule;

#[tokio::test]
async fn with_no_unit_of_work_bound_a_hook_runs_bare() {
    let logs = LogCapture::install();
    let app = App::new::<BareWitnessModule>().expect("the module boots");
    app.init().await.expect("the hook runs bare");
    assert_eq!(
        hook_line(&logs, "init").field("outcome").as_deref(),
        Some(operation_log::OK),
    );
}
