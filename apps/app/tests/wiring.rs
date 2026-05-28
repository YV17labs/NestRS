use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nestrs_core::{injectable, module, App, Container, ContainerBuilder, DynamicModule, Module};

#[injectable]
#[derive(Default)]
struct Dependency;

impl Dependency {
    fn value(&self) -> u32 {
        21
    }
}

#[injectable]
struct Consumer {
    #[inject]
    dep: Arc<Dependency>,
}

impl Consumer {
    fn doubled(&self) -> u32 {
        self.dep.value() * 2
    }
}

#[module(providers = [Consumer, Dependency])]
struct ReversedModule;

#[test]
fn provider_listed_before_its_dependency_still_resolves() {
    let container = ReversedModule::register(Container::builder()).build();
    let consumer: Arc<Consumer> = container.get().expect("Consumer resolves");
    assert_eq!(consumer.doubled(), 42);
}

#[injectable]
struct Orphan {
    #[inject]
    _missing: Arc<Dependency>,
}

#[module(providers = [Orphan])]
struct BrokenModule;

#[test]
#[should_panic(expected = "Orphan (needs Dependency)")]
fn missing_dependency_panics_with_a_clear_message() {
    // Orphan injects a Dependency no provider registers: the "missing provider"
    // branch of the fixpoint names the absent dependency type, kept distinct
    // from a cycle.
    let _ = BrokenModule::register(Container::builder()).build();
}

#[injectable]
struct Yin {
    #[inject]
    _yang: Arc<Yang>,
}

#[injectable]
struct Yang {
    #[inject]
    _yin: Arc<Yin>,
}

// Each injects the other (`Arc` breaks the recursion at the type level), so the
// fixpoint can build neither — a genuine dependency cycle, reported as such
// rather than as a missing provider.
#[module(providers = [Yin, Yang])]
struct CyclicModule;

#[test]
#[should_panic(expected = "dependency cycle")]
fn mutual_dependency_is_reported_as_a_cycle() {
    let _ = CyclicModule::register(Container::builder()).build();
}

// --- Module registration is idempotent (diamond imports) ---

static SHARED_BUILDS: AtomicUsize = AtomicUsize::new(0);

// `Tick::default` bumps the build counter; held as a field so constructing the
// provider below increments it once. (A unit `#[injectable]` builds via `Self`,
// not `Default`, so the counter rides on a field instead.)
struct Tick;

impl Default for Tick {
    fn default() -> Self {
        SHARED_BUILDS.fetch_add(1, Ordering::SeqCst);
        Tick
    }
}

// A provider that counts how many times it is constructed, so a diamond import
// can assert its module's providers are built exactly once.
#[injectable]
#[derive(Default)]
struct Counted {
    _tick: Tick,
}

#[module(providers = [Counted])]
struct SharedModule;

#[module(imports = [SharedModule])]
struct LeftModule;

#[module(imports = [SharedModule])]
struct RightModule;

// Both arms import SharedModule — a diamond. Without dedup, Counted would build
// twice (and the container would log an override warning).
#[module(imports = [LeftModule, RightModule])]
struct DiamondRoot;

#[test]
fn diamond_import_builds_shared_provider_once() {
    SHARED_BUILDS.store(0, Ordering::SeqCst);
    let _ = DiamondRoot::register(Container::builder()).build();
    assert_eq!(SHARED_BUILDS.load(Ordering::SeqCst), 1);
}

// --- Dynamic modules carry sync config from the import site (forRoot) ---

struct ConfigValue(u32);

struct ConfiguredModule;

impl ConfiguredModule {
    fn for_root(n: u32) -> ConfiguredSetup {
        ConfiguredSetup(n)
    }
}

struct ConfiguredSetup(u32);

impl DynamicModule for ConfiguredSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide(ConfigValue(self.0))
    }
}

// A call expression in `imports` is registered as a dynamic module by value.
#[module(imports = [ConfiguredModule::for_root(99)])]
struct DynRoot;

#[test]
fn dynamic_module_carries_sync_config_from_import_site() {
    let container = DynRoot::register(Container::builder()).build();
    assert_eq!(container.get::<ConfigValue>().unwrap().0, 99);
}

// --- Dynamic modules own an async factory via collect (forRootAsync) ---

struct AsyncValue(u32);

struct AsyncConfigModule;

impl AsyncConfigModule {
    fn for_root(n: u32) -> AsyncConfigSetup {
        AsyncConfigSetup(n)
    }
}

struct AsyncConfigSetup(u32);

impl DynamicModule for AsyncConfigSetup {
    // Owns its provider asynchronously: the factory is queued in collect and
    // awaited by `App::builder().build()` before providers are built.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let n = self.0;
        builder.provide_factory(move |_| async move { Ok(AsyncValue(n)) })
    }
}

// Proves the `#[module]` macro generates a `collect` that recurses into a
// dynamic import and queues its factory — the path the `DatabaseModule` uses.
#[module(imports = [AsyncConfigModule::for_root(123)])]
struct AsyncRoot;

#[tokio::test]
async fn dynamic_module_factory_runs_via_macro_collect() {
    let app = App::builder()
        .module::<AsyncRoot>()
        .build()
        .await
        .expect("build succeeds");
    assert_eq!(app.container().get::<AsyncValue>().unwrap().0, 123);
}

// --- Optional dependencies (`#[inject] Option<Arc<T>>`, the @Optional analog) ---

#[injectable]
#[derive(Default)]
struct Extra;

impl Extra {
    fn tag(&self) -> &'static str {
        "present"
    }
}

#[injectable]
struct MaybeConsumer {
    #[inject]
    extra: Option<Arc<Extra>>,
}

impl MaybeConsumer {
    fn report(&self) -> &'static str {
        self.extra.as_ref().map(|e| e.tag()).unwrap_or("absent")
    }
}

// Extra is not provided: the optional dependency resolves to `None`, and — the
// point — the access-graph check does not fail the boot over an absent optional.
#[module(providers = [MaybeConsumer])]
struct OptionalAbsentModule;

#[test]
fn optional_dependency_is_none_when_absent_and_does_not_fail_boot() {
    let app = App::new::<OptionalAbsentModule>().expect("boots");
    let consumer: Arc<MaybeConsumer> = app.container().get().expect("MaybeConsumer resolves");
    assert_eq!(consumer.report(), "absent");
}

// Extra is provided *after* the consumer in the list: the fixpoint still orders
// the consumer last, so the optional resolves to `Some` regardless of order.
#[module(providers = [MaybeConsumer, Extra])]
struct OptionalPresentModule;

#[test]
fn optional_dependency_is_some_when_provided_regardless_of_order() {
    let app = App::new::<OptionalPresentModule>().expect("boots");
    let consumer: Arc<MaybeConsumer> = app.container().get().expect("MaybeConsumer resolves");
    assert_eq!(consumer.report(), "present");
}
