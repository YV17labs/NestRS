//! Covers `src/module.rs` — a `#[process]` host the app reaches without
//! `QueueModule` is named at run, with the import that serves it, and one the
//! worker serves is not.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, AppLifecycle, injectable, module, target};
use nest_rs_queue::{BoundConsumer, Jobs, QueueModule, processor, queue};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

use crate::memory::{Memory, PLAIN};

/// What `App::run` files for a host whose edge's transport the app does not attach.
const UNSERVED: &str = "host not served: its edge's transport is not attached";

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

/// A hand-written adapter that forgets the worker.
#[module(providers = [ReindexProcessor])]
struct ReindexQueueModule;

#[tokio::test]
async fn a_process_host_the_app_reaches_without_the_worker_is_named_at_run() {
    let logs = LogCapture::install();

    App::builder()
        .module::<ReindexQueueModule>()
        .build()
        .await
        .expect("the module boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(target::APP, UNSERVED);
    assert_eq!(unserved.level, "warn");
    assert_eq!(unserved.field("edge").as_deref(), Some("queue"));
    assert_eq!(unserved.field("host").as_deref(), Some("ReindexProcessor"));
    assert_eq!(
        unserved.field("module").as_deref(),
        Some("ReindexQueueModule")
    );
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("nest_rs::queue::QueueModule")
    );
}

/// The processor's adapter beside the worker.
#[module(imports = [QueueModule, ReindexQueueModule])]
struct ServedReindexModule;

#[tokio::test]
async fn a_process_host_the_worker_serves_is_not_named_at_run() {
    let logs = LogCapture::install();
    let memory = Memory::new(&PLAIN, Duration::from_secs(30));
    let producer = Jobs::new(Arc::new(memory.clone()));
    let app = App::builder()
        .provide(BoundConsumer::new(memory.consumer()))
        .provide(producer)
        .module::<ServedReindexModule>()
        .build()
        .await
        .expect("the module boots over the in-memory backend");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");

    // `App::run` names an unserved host before it turns ready, so the run is
    // dropped there.
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
    logs.expect_none(target::APP, UNSERVED);
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
        .module::<ServedReindexModule>()
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
}
