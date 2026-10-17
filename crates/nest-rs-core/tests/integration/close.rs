//! Covers `src/close.rs` — the resources a module opened close after every
//! shutdown hook, last opened first, within the hooks' budget, and the
//! container's drop after them is bounded too.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::{
    App, ContainerBuilder, Module, Registering, SHUTDOWN_HOOKS_TIMEOUT, StopCause, hooks,
    injectable, module, target,
};
use nest_rs_testing::LogCapture;

static ENDED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn ended(entry: &'static str) {
    ENDED.lock().expect("log mutex is not poisoned").push(entry);
}

struct Pool;
struct Cache;

/// Opens a pool, then a cache built over it, and closes both.
struct ResourcesModule;

impl Module for ResourcesModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide(Pool)
            .provide_close::<Pool, _, _>(|_| async {
                ended("Pool closed");
                Ok(())
            })
            .provide(Cache)
            .provide_close::<Cache, _, _>(|_| async {
                ended("Cache closed");
                Ok(())
            })
    }
}

#[injectable]
#[derive(Default)]
struct UsesTheCache;

#[hooks]
impl UsesTheCache {
    #[on_module_init]
    async fn warm(&self) {
        ended("UsesTheCache warmed");
    }

    #[on_application_shutdown]
    async fn flush(&self) {
        ended("UsesTheCache flushed");
    }
}

#[module(imports = [ResourcesModule], providers = [UsesTheCache])]
struct ClosesModule;

#[tokio::test]
async fn resources_close_after_every_shutdown_hook_last_opened_first() {
    App::new::<ClosesModule>()
        .expect("boots")
        .run()
        .await
        .expect("an app serving nothing stops at once");

    assert_eq!(
        *ENDED.lock().expect("log mutex is not poisoned"),
        [
            "UsesTheCache warmed",
            "UsesTheCache flushed",
            "Cache closed",
            "Pool closed",
        ],
    );
}

static NEVER_INIT: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn never_init(entry: &'static str) {
    NEVER_INIT
        .lock()
        .expect("log mutex is not poisoned")
        .push(entry);
}

#[injectable]
#[derive(Default)]
struct Registers;

#[hooks]
impl Registers {
    #[on_module_init]
    async fn register(&self) {
        never_init("registered");
    }

    #[on_module_destroy]
    async fn deregister(&self) {
        never_init("deregistered");
    }
}

struct Connection;

struct ConnectionModule;

impl Module for ConnectionModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide(Connection)
            .provide_close::<Connection, _, _>(|_| async {
                never_init("connection closed");
                Ok(())
            })
    }
}

#[module(imports = [ConnectionModule], providers = [Registers])]
struct NeverInitModule;

#[tokio::test]
async fn closing_an_app_whose_init_never_ran_runs_no_hook_but_closes_what_it_opened() {
    let app = App::new::<NeverInitModule>().expect("boots");
    let lifecycle = Arc::clone(app.lifecycle());

    app.close().await;

    assert_eq!(
        *NEVER_INIT.lock().expect("log mutex is not poisoned"),
        ["connection closed"],
        "no hook ran, so none is torn down; the connection was open all the same",
    );
    assert_eq!(lifecycle.cause(), Some(StopCause::Closed));
}

struct Socket;

struct HangsModule;

impl Module for HangsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
            .provide(Socket)
            .provide_close::<Socket, _, _>(|_| std::future::pending())
    }
}

#[tokio::test(start_paused = true)]
async fn a_close_that_never_returns_is_abandoned_at_the_budget_naming_it() {
    let logs = LogCapture::install();
    let started = tokio::time::Instant::now();

    App::new::<HangsModule>().expect("boots").close().await;

    let took = started.elapsed();
    assert!(
        took >= SHUTDOWN_HOOKS_TIMEOUT && took < SHUTDOWN_HOOKS_TIMEOUT + Duration::from_secs(1),
        "the close waited for the budget and no longer, took {took:?}",
    );
    let abandoned = logs.expect_one(
        target::LIFECYCLE,
        "resource close abandoned: the shutdown hooks' budget was spent while it waited, and \
         the closes after it still start",
    );
    assert_eq!(abandoned.level, "warn");
    assert_eq!(abandoned.field("close").as_deref(), Some("Socket"));
}

/// Says it began dropping, then blocks its thread until the test releases it.
struct BlocksOnDrop {
    dropping: Arc<tokio::sync::Notify>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Drop for BlocksOnDrop {
    fn drop(&mut self) {
        self.dropping.notify_one();
        let _ = self
            .release
            .lock()
            .expect("the receiver's mutex is not poisoned")
            .recv();
    }
}

#[tokio::test(start_paused = true)]
async fn a_provider_whose_drop_blocks_is_abandoned_at_the_budget() {
    let logs = LogCapture::install();
    let dropping = Arc::new(tokio::sync::Notify::new());
    let (release, released) = std::sync::mpsc::channel();
    let app = App::builder()
        .provide(BlocksOnDrop {
            dropping: Arc::clone(&dropping),
            release: Mutex::new(released),
        })
        .build()
        .await
        .expect("boots");

    let closing = tokio::spawn(app.close());
    dropping.notified().await;
    tokio::time::advance(SHUTDOWN_HOOKS_TIMEOUT).await;
    closing
        .await
        .expect("the close returned while the drop still blocks");

    let abandoned = logs.expect_one(
        target::APP,
        "a provider was still dropping when the shutdown hooks' budget ran out: abandoned",
    );
    assert_eq!(abandoned.level, "warn");
    release.send(()).expect("the drop still waits");
}

static FAILED_BUILD: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

struct Lease;
struct Index;

/// Closes the lease its app's factory opens, and attaches the close while it
/// collects, before any factory runs.
struct LeaseModule;

impl Module for LeaseModule {
    fn collect(builder: ContainerBuilder, _: nest_rs_core::Collecting<Self>) -> ContainerBuilder {
        builder.provide_close::<Lease, _, _>(|_| async {
            FAILED_BUILD
                .lock()
                .expect("log mutex is not poisoned")
                .push("lease released");
            Ok(())
        })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[tokio::test]
async fn a_boot_whose_later_factory_fails_closes_what_an_earlier_one_opened() {
    let refused = App::builder()
        .provide_factory(|_| async { Ok(Lease) })
        .provide_factory(|_| async { Err::<Index, _>(anyhow::anyhow!("the index is corrupt")) })
        .module::<LeaseModule>()
        .build()
        .await
        .err()
        .expect("the second factory fails the boot");

    assert!(
        refused
            .downcast_ref::<nest_rs_core::FactoryFailedError>()
            .is_some(),
        "{refused:#}"
    );
    assert_eq!(
        *FAILED_BUILD.lock().expect("log mutex is not poisoned"),
        ["lease released"],
    );
}

static SEEDED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

struct Handle;

/// Closes the handle it would have opened.
struct HandleModule;

impl Module for HandleModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_close::<Handle, _, _>(|_| async {
            SEEDED
                .lock()
                .expect("log mutex is not poisoned")
                .push("handle closed");
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_resource_the_app_was_handed_is_left_for_the_app_to_close() {
    let app = App::builder()
        .provide(Handle)
        .module::<HandleModule>()
        .build()
        .await
        .expect("boots");
    let handle = app.container().get::<Handle>();

    app.close().await;

    assert!(handle.is_some());
    assert!(
        SEEDED.lock().expect("log mutex is not poisoned").is_empty(),
        "the seed is the test's, never the module's to close"
    );
}

static SIBLINGS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn sibling_closed(entry: &'static str) {
    SIBLINGS
        .lock()
        .expect("log mutex is not poisoned")
        .push(entry);
}

struct BaseConnection;
struct ReplicaConnection;

/// Opens a replica over the base connection, attaching its close beside its
/// factory.
struct ReplicaModule;

impl Module for ReplicaModule {
    fn collect(builder: ContainerBuilder, _: nest_rs_core::Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory_after::<ReplicaConnection, (BaseConnection,), _, _>(|_| async {
                Ok(ReplicaConnection)
            })
            .provide_close::<ReplicaConnection, _, _>(|_| async {
                sibling_closed("ReplicaConnection closed");
                Ok(())
            })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

/// Opens the base connection, attaching its close beside its factory.
struct BaseModule;

impl Module for BaseModule {
    fn collect(builder: ContainerBuilder, _: nest_rs_core::Collecting<Self>) -> ContainerBuilder {
        builder
            .provide_factory(|_| async { Ok(BaseConnection) })
            .provide_close::<BaseConnection, _, _>(|_| async {
                sibling_closed("BaseConnection closed");
                Ok(())
            })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[module(imports = [ReplicaModule, BaseModule])]
struct SiblingsModule;

#[tokio::test]
async fn a_resource_opened_over_another_closes_before_it_whatever_the_import_order() {
    App::builder()
        .module::<SiblingsModule>()
        .build()
        .await
        .expect("boots")
        .close()
        .await;

    assert_eq!(
        *SIBLINGS.lock().expect("log mutex is not poisoned"),
        ["ReplicaConnection closed", "BaseConnection closed"],
        "the replica's close was attached first, yet its factory opened it last",
    );
}
