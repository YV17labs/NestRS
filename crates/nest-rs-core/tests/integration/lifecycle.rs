//! The fourth shape a lifecycle host can take, and the only one left at
//! runtime: bound as `dyn Trait`.
//!
//! [`ProviderResidency`](nest_rs_core::ProviderResidency) refuses the three shapes no
//! composition can fix (an edge host, `scope = request`, `scope = transient`)
//! at compile time. This one is the app's composition and the app can fix it:
//! `providers = [Foo as dyn Trait]` stores `Arc<dyn Trait>`, so nothing sits
//! under `Foo`, which is what the decorator resolves. Both halves are asserted
//! here — that it skips, and that listing the host both ways **constructs it
//! twice**. The second is why the skip line names causes and prescribes no
//! edit: double-listing is the repair it looks like, and is not one.

use nest_rs_core::target;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_core::{App, SHUTDOWN_HOOK_TIMEOUT, hooks, injectable, module};
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

/// Counts constructions. The first version of this file used a unit struct and
/// asserted only that the hook *ran*, which is true with one instance or two —
/// so it passed while claiming double-listing is a fix.
static BUILDS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct BothWaysHost {
    // Nothing reads it. `#[injectable]` builds a *unit* struct as a bare `Self`
    // and a named-field one through `Default`, so only the second can count.
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

/// **Why the skip line prescribes no edit.** Listing a host both ways looks like
/// the obvious repair and reads as one, but each binding runs the constructor:
/// the decorators fire on the concrete instance while every consumer injecting
/// `Arc<dyn Bridge>` holds a different one, and nothing anywhere says so —
/// `DuplicateProviderError` cannot fire, because the two container keys differ.
/// A hint that recommended this shipped for exactly one audit round.
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

/// A shutdown hook that fails is **not** propagated — and the event is the
/// whole of what is left.
///
/// The split is deliberate: an init failure aborts the boot and reaches `main`
/// as an error, so it needs no log to be noticed. Shutdown is best-effort, so
/// one provider's failed cleanup must not skip another's — which means the
/// error is swallowed by design, and `lifecycle hook failed` is the only record
/// that a connection pool, a flush or a lock release did not happen.
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
    // No transports, so `run` drains the serve loop immediately and goes
    // straight to the shutdown phases — which is the only public path to them.
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
    #[allow(clippy::needless_arbitrary_self_type, clippy::unused_unit)]
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
#[allow(non_camel_case_types)]
struct r#async;

#[hooks]
impl r#async {
    #[on_module_init]
    async fn init(&self) {}
}

#[module(providers = [ShapedHost])]
struct ShapedModule;

/// Compiling is the first half: a hook compiled out takes its entry with it.
/// `-> ()` written out is the infallible shape — it was read as a `Result`, and
/// its `()` handed to `map_err` — `self: &Self` is `&self` spelled out,
/// `self: &Arc<Self>` borrows what the container holds, and a raw identifier is
/// labelled by its name: a method's, which the run order sorts on, and a host's.
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

/// A trait with a method of a hook's name, implemented for every `Arc<T>` — the
/// shape an extension trait takes.
#[allow(dead_code)]
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

/// The provider a hook runs on is an `Arc<Host>`, and method-call syntax looks a
/// name up on the `Arc` first: a trait in scope with a method of the hook's name
/// implemented for `Arc<T>` ran in the hook's place, and the hook never ran.
#[tokio::test]
async fn a_hook_runs_where_a_trait_on_arc_shares_its_name() {
    let app = App::new::<WarmModule>().expect("the module boots");
    app.init().await.expect("the init phases drain");
    assert_eq!(WARMED.load(Ordering::SeqCst), 1);
}

/// Every shutdown hook that ran after the stuck one.
static TIDIED: AtomicUsize = AtomicUsize::new(0);

/// A cleanup that never returns — a flush waiting on a peer that went away.
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

/// A shutdown hook that never returns used to hold the process until the
/// orchestrator killed it — skipping every hook after it and the telemetry
/// flush. It is abandoned at its bound, named at `warn`, and the rest still run.
#[tokio::test(start_paused = true)]
async fn a_shutdown_hook_that_never_returns_is_abandoned_at_its_bound_and_the_rest_still_run() {
    let logs = LogCapture::install();
    let started = tokio::time::Instant::now();
    App::new::<StuckOnDestroyModule>()
        .expect("the module boots")
        .run()
        .await
        .expect("an abandoned cleanup is not an error: shutdown is best-effort");
    let took = started.elapsed();

    assert!(
        took >= SHUTDOWN_HOOK_TIMEOUT && took < SHUTDOWN_HOOK_TIMEOUT + Duration::from_secs(1),
        "shutdown waited on the stuck hook for its bound and no longer, took {took:?}",
    );
    assert_eq!(
        TIDIED.load(Ordering::SeqCst),
        3,
        "the hook after it in its phase, and one in each later phase, all ran",
    );
    let event = logs.expect_one(
        target::LIFECYCLE,
        "shutdown hook abandoned: it did not return within the bound, and the hooks after it \
         still run",
    );
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
        Some(SHUTDOWN_HOOK_TIMEOUT.as_millis().to_string()),
    );
}
