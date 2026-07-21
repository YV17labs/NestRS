//! Dynamic imports (`Foo::for_root(opts)`) through the real `#[module]` macro
//! and both boot paths. The contract under test is CORE-I9: the import
//! expression is evaluated **exactly once**, and the value the collect phase
//! saw is the value the register phase installs.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{App, ContainerBuilder, DynamicModule, module};

/// Counts how many times the import expression ran, and stamps each
/// construction with a serial so a test can tell *which* value was installed.
static BUILDS: AtomicUsize = AtomicUsize::new(0);

/// What a dynamic module installs — the serial of the value that reached
/// `register`, resolvable from the built container.
struct Installed(usize);

/// The value collected, so a test can compare it against the registered one.
struct Collected(usize);

struct CountingSetup {
    serial: usize,
}

/// The non-idempotent `for_root` the old contract forbade: every call bumps the
/// counter. Under CORE-I9 it may be impure, because it runs once.
fn for_root() -> CountingSetup {
    CountingSetup {
        serial: BUILDS.fetch_add(1, Ordering::SeqCst),
    }
}

impl DynamicModule for CountingSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide(Installed(self.serial))
    }

    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide(Collected(self.serial))
    }
}

#[module(imports = [for_root()])]
struct CountingModule;

#[tokio::test]
async fn a_dynamic_import_is_evaluated_once_on_the_async_path() {
    BUILDS.store(0, Ordering::SeqCst);

    let app = App::builder()
        .module::<CountingModule>()
        .build()
        .await
        .expect("the module boots");

    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        1,
        "collect + register must share one construction of the import expression",
    );
    // Same value across both phases: whatever `collect` saw is what `register`
    // installed. A re-evaluated expression would show two different serials.
    let collected: Arc<Collected> = app.container().get().expect("collect ran");
    let installed: Arc<Installed> = app.container().get().expect("register ran");
    assert_eq!(collected.0, installed.0);
}

struct SyncSetup;

impl DynamicModule for SyncSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide(Installed(BUILDS.fetch_add(1, Ordering::SeqCst)))
    }
}

fn sync_for_root() -> SyncSetup {
    BUILDS.fetch_add(1, Ordering::SeqCst);
    SyncSetup
}

#[module(imports = [sync_for_root()])]
struct SyncModule;

#[test]
fn a_dynamic_import_is_evaluated_once_on_the_sync_path() {
    // `App::new` has no collect phase, so nothing is parked and `register`
    // falls back to building the value itself — still exactly once.
    BUILDS.store(0, Ordering::SeqCst);

    let app = App::new::<SyncModule>().expect("the module boots");

    assert!(
        app.container().get::<Installed>().is_some(),
        "the fallback path must still register the dynamic module",
    );
    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        2,
        "one construction (+1) then one register (+1) — never a second construction",
    );
}

struct Tagged(&'static str);

struct TaggingSetup(&'static str);

impl DynamicModule for TaggingSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_keyed(self.0, Tagged(self.0))
    }
}

#[module(imports = [TaggingSetup("first"), TaggingSetup("second")])]
struct TwoDynamicImportsModule;

#[tokio::test]
async fn two_dynamic_imports_in_one_module_keep_their_own_values() {
    // Parked values are keyed by (module, import index): two sites in the same
    // module must not clobber each other, and each must reach `register`.
    let app = App::builder()
        .module::<TwoDynamicImportsModule>()
        .build()
        .await
        .expect("the module boots");

    assert_eq!(
        app.container().get_keyed::<Tagged>("first").unwrap().0,
        "first"
    );
    assert_eq!(
        app.container().get_keyed::<Tagged>("second").unwrap().0,
        "second"
    );
}

struct Mixed(usize);

struct MixedSetup;

impl DynamicModule for MixedSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide(Mixed(7))
    }
}

#[module]
struct StaticLeaf;

// A static import *before* the dynamic one: the collect and register passes
// must agree on the dynamic import's index despite the static entry sharing
// the same list.
#[module(imports = [StaticLeaf, MixedSetup {}, StaticLeaf])]
struct MixedImportsModule;

#[tokio::test]
async fn a_dynamic_import_after_a_static_one_still_resolves_its_site() {
    let app = App::builder()
        .module::<MixedImportsModule>()
        .build()
        .await
        .expect("the module boots");

    assert_eq!(
        app.container()
            .get::<Mixed>()
            .expect("dynamic import ran")
            .0,
        7,
        "a mismatched collect/register index would leave the value parked",
    );
}
