//! Covers `src/module.rs` — a listed `#[process]` host brings `QueueModule`, so
//! the app importing its adapter attaches the worker, and a host whose methods
//! are all compiled out brings nothing.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, AppLifecycle, Module, injectable, module, target};
use nest_rs_queue::{BoundConsumer, Jobs, QueueConfig, QueueModule, processor, queue};
use nest_rs_testing::{HeadlessApp, LogCapture};
use serde::{Deserialize, Serialize};

use crate::memory::{Memory, PLAIN};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReindexCommand {
    id: u32,
}

#[queue(name = "nestrs-module-reindex", job = ReindexCommand)]
struct ReindexQueue;

#[injectable]
#[derive(Default)]
struct ReindexProcessor;

#[processor]
impl ReindexProcessor {
    #[process(queue = ReindexQueue)]
    async fn reindex(&self, _job: ReindexCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

/// An adapter as a developer writes it: the processor, and nothing of the
/// edge's.
#[module(providers = [ReindexProcessor])]
struct ReindexQueueModule;

/// The app `M` composes, booted with no transport started.
async fn booted<M: Module>() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the module boots")
}

#[tokio::test]
async fn a_listed_process_host_brings_the_worker_and_reads_its_config() {
    let app = booted::<ReindexQueueModule>().await;

    assert!(
        app.container().get::<QueueConfig>().is_some(),
        "the bare import reads the edge's variables",
    );
    assert_eq!(app.attached_transports(), ["queue"]);
}

fn pinned() -> QueueConfig {
    QueueConfig {
        shutdown_timeout: Duration::from_secs(7),
        ..QueueConfig::default()
    }
}

#[module(imports = [QueueModule::for_root(pinned()), ReindexQueueModule])]
struct PinnedBeforeModule;

#[module(imports = [ReindexQueueModule, QueueModule::for_root(pinned())])]
struct PinnedAfterModule;

#[tokio::test]
async fn a_pin_supersedes_what_a_process_host_brings_on_either_side() {
    for app in [
        booted::<PinnedBeforeModule>().await,
        booted::<PinnedAfterModule>().await,
    ] {
        let config = app
            .container()
            .get::<QueueConfig>()
            .expect("the pin is read");
        assert_eq!(config.shutdown_timeout, Duration::from_secs(7));
        assert_eq!(app.attached_transports(), ["queue"]);
    }
}

#[injectable]
#[derive(Default)]
struct DisabledProcessor;

#[processor]
impl DisabledProcessor {
    #[cfg(any())]
    #[process(queue = ReindexQueue)]
    async fn reindex(&self, _job: ReindexCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(providers = [DisabledProcessor])]
struct DisabledQueueModule;

/// A worker with nothing to run would refuse an app binding no backend.
#[tokio::test]
async fn a_process_host_whose_methods_are_compiled_out_brings_no_worker() {
    assert!(
        booted::<DisabledQueueModule>()
            .await
            .attached_transports()
            .is_empty()
    );
}

#[tokio::test]
async fn a_listed_process_host_is_served_over_the_bound_backend() {
    let logs = LogCapture::install();
    let memory = Memory::new(&PLAIN, Duration::from_secs(30));
    let producer = Jobs::new(Arc::new(memory.clone()));
    let app = App::builder()
        .provide(BoundConsumer::new(memory.consumer()))
        .provide(producer)
        .module::<ReindexQueueModule>()
        .build()
        .await
        .expect("the module boots over the in-memory backend");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");

    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
}

/// A `#[process]` method the worker serves is one `mounted` line naming its
/// host, its method and its queue, as every edge's unit line does.
#[tokio::test]
async fn a_process_method_the_worker_serves_is_mounted_naming_its_host_and_method() {
    let logs = LogCapture::install();
    let memory = Memory::new(&PLAIN, Duration::from_secs(30));
    let app = App::builder()
        .provide(BoundConsumer::new(memory.consumer()))
        .provide(Jobs::new(Arc::new(memory.clone())))
        .module::<ReindexQueueModule>()
        .build()
        .await
        .expect("the module boots over the in-memory backend");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    let mounted = logs.expect_one(nest_rs_queue::TARGET, "mounted");
    assert_eq!(mounted.field("host").as_deref(), Some("ReindexProcessor"));
    assert_eq!(mounted.field("method").as_deref(), Some("reindex"));
    assert_eq!(
        mounted.field("queue").as_deref(),
        Some("nestrs-module-reindex")
    );

    // Every other `#[processor]` this suite links is one this app does not reach.
    let skipped: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|line| line.message.starts_with("skipped #[process] method"))
        .collect();
    assert!(!skipped.is_empty(), "the suite links other processors");
    for line in skipped {
        assert!(line.field("host").is_some(), "{:?}", line.fields);
        assert!(line.field("method").is_some(), "{:?}", line.fields);
        assert_eq!(line.field("processor"), None, "{:?}", line.fields);
    }
}

#[tokio::test]
async fn a_process_host_seeded_with_no_worker_is_named_at_run() {
    let logs = LogCapture::install();
    App::builder()
        .provide(ReindexProcessor)
        .build()
        .await
        .expect("the app boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(
        target::APP,
        "host not served: its edge's transport is not attached",
    );
    assert_eq!(unserved.field("host").as_deref(), Some("ReindexProcessor"));
    assert_eq!(unserved.field("import").as_deref(), Some("QueueModule"));
}

#[module(imports = [QueueModule])]
struct WorkerAlone;

/// Following that line serves the seed: the worker reads what the app reaches,
/// and a seed is reached.
#[tokio::test]
async fn a_process_host_seeded_beside_the_worker_is_served() {
    let logs = LogCapture::install();
    let memory = Memory::new(&PLAIN, Duration::from_secs(30));
    let app = App::builder()
        .provide(ReindexProcessor)
        .provide(BoundConsumer::new(memory.consumer()))
        .provide(Jobs::new(Arc::new(memory.clone())))
        .module::<WorkerAlone>()
        .build()
        .await
        .expect("the module boots over the in-memory backend");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    let mounted = logs.expect_one(nest_rs_queue::TARGET, "mounted");
    assert_eq!(mounted.field("host").as_deref(), Some("ReindexProcessor"));
    logs.expect_none(
        target::APP,
        "host not served: its edge's transport is not attached",
    );
}
