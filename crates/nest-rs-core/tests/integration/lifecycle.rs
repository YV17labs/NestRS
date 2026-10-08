//! Covers `src/lifecycle.rs` — hook shapes, a host bound only as `dyn Trait`,
//! and shutdown hooks that fail, hang or panic.

use nest_rs_core::target;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{App, SHUTDOWN_HOOKS_TIMEOUT, hooks, injectable, module};
use nest_rs_testing::LogCapture;

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

static BUILDS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct BothWaysHost {
    // `#[injectable]` builds a unit struct as a bare `Self`, bypassing `Default`.
    _counted: (),
}

impl Default for BothWaysHost {
    fn default() -> Self {
        BUILDS.fetch_add(1, Ordering::SeqCst);
        Self { _counted: () }
    }
}

impl Bridge for BothWaysHost {}

#[hooks]
impl BothWaysHost {
    #[on_module_init]
    async fn init(&self) {}
}

#[module(providers = [BothWaysHost, BothWaysHost as dyn Bridge])]
struct BothWaysModule;

#[tokio::test]
async fn a_host_bound_only_as_dyn_never_fires() {
    let app = App::new::<DynOnlyModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");

    assert_eq!(
        DYN_ONLY.load(Ordering::SeqCst),
        0,
        "the container holds `Arc<dyn Bridge>`, never a `DynOnlyHost` to call",
    );
}

#[tokio::test]
async fn listing_a_host_both_ways_builds_it_twice() {
    let app = App::new::<BothWaysModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");

    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        2,
        "each binding constructs its own instance — the decorators fire on one, \
         every `Arc<dyn Bridge>` consumer holds the other",
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
    let mut methods: Vec<&str> = nest_rs_core::inventory::iter::<nest_rs_core::LifecycleHook>()
        .filter(|hook| hook.provider == "ShapedHost")
        .map(|hook| hook.method)
        .collect();
    methods.sort_unstable();
    assert_eq!(methods, ["init", "through_its_arc", "type"]);
    let raw_hosts = nest_rs_core::inventory::iter::<nest_rs_core::LifecycleHook>()
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

#[injectable]
#[derive(Default)]
struct StuckOnDestroy;

#[hooks]
impl StuckOnDestroy {
    #[on_module_destroy]
    async fn flush(&self) {
        std::future::pending::<()>().await;
    }
}

/// Sorts after `StuckOnDestroy` in their shared phase, and has a hook in each
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

#[injectable]
#[derive(Default)]
struct AaPanicsOnDestroy;

#[hooks]
impl AaPanicsOnDestroy {
    #[on_module_destroy]
    async fn release(&self) {
        panic!("the pool was already closed");
    }
}

/// Sorts after `AaPanicsOnDestroy` in its phase, and has a hook in a later one.
#[injectable]
#[derive(Default)]
struct ZzAfterPanic;

#[hooks]
impl ZzAfterPanic {
    #[on_module_destroy]
    async fn release(&self) {
        AFTER_PANIC.fetch_add(1, Ordering::SeqCst);
    }

    #[on_application_shutdown]
    async fn close(&self) {
        AFTER_PANIC.fetch_add(1, Ordering::SeqCst);
    }
}

#[module(providers = [AaPanicsOnDestroy, ZzAfterPanic])]
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
            .is_some_and(|p| p.contains("AaPanicsOnDestroy")),
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
        "lifecycle hook PanicsOnInit::warm (OnModuleInit) panicked",
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
