//! Covers `src/module.rs` — a dynamic import (`Foo::for_root(opts)`) is
//! evaluated exactly once, and the value collected is the value registered.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    App, Collecting, ContainerBuilder, DynamicModule, LateFactoryError, Module, Registering, module,
};

static BUILDS: AtomicUsize = AtomicUsize::new(0);

struct Installed(usize);

struct Collected(usize);

struct CountingSetup {
    serial: usize,
}

fn for_root() -> CountingSetup {
    CountingSetup {
        serial: BUILDS.fetch_add(1, Ordering::SeqCst),
    }
}

impl DynamicModule for CountingSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Installed(self.serial))
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
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
    let collected: Arc<Collected> = app.container().get().expect("collect ran");
    let installed: Arc<Installed> = app.container().get().expect("register ran");
    assert_eq!(collected.0, installed.0);
}

struct SyncSetup;

impl DynamicModule for SyncSetup {
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
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
    BUILDS.store(0, Ordering::SeqCst);

    let app = App::new::<SyncModule>().expect("the module boots");

    assert!(
        app.container().get::<Installed>().is_some(),
        "the synchronous path registers the dynamic module",
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
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_keyed(self.0, Tagged(self.0))
    }
}

#[module(imports = [TaggingSetup("first"), TaggingSetup("second")])]
struct TwoDynamicImportsModule;

#[tokio::test]
async fn two_dynamic_imports_in_one_module_keep_their_own_values() {
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
    fn module() -> TypeId {
        TypeId::of::<Self>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Mixed(7))
    }
}

#[module]
struct StaticLeaf;

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

struct Opened;

struct OpensModule;

impl Module for OpensModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_factory(|_| async { Ok(Opened) })
    }
}

#[module(imports = [OpensModule])]
struct OpeningModule;

struct UncollectingSetup;

impl DynamicModule for UncollectingSetup {
    fn module() -> TypeId {
        TypeId::of::<OpeningModule>()
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<OpeningModule>()
    }
}

#[module(imports = [UncollectingSetup {}])]
struct UncollectedModule;

#[tokio::test]
async fn a_module_imported_in_a_register_alone_fails_the_boot_naming_what_it_opens() {
    let Err(refused) = App::builder().module::<UncollectedModule>().build().await else {
        panic!("the factory its import queues would never run");
    };
    let late = refused
        .downcast_ref::<LateFactoryError>()
        .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
    assert!(late.type_name.ends_with("Opened"), "{late:?}");
}
