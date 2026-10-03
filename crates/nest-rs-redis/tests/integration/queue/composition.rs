//! `RedisQueueModule`'s composition, in process: the portable producer name is a
//! *declaration*, so a second queue backend imported beside it fails the boot
//! naming the remedy — before any factory runs, so before Redis is dialled.

use std::sync::Arc;

use nest_rs_core::{App, ContainerBuilder, Module, module};
use nest_rs_queue::{
    Capabilities, Envelope, JobProducer, PushOptions, QueueBackend, QueueError, QueueName,
};
use nest_rs_redis::{RedisConfig, RedisModule, RedisQueueModule};

static ELSEWHERE: QueueBackend = QueueBackend::new("elsewhere", Capabilities::NONE);

/// Another backend's producer — what a second queue binding would declare.
struct ElsewhereProducer;

#[nest_rs_queue::async_trait]
impl JobProducer for ElsewhereProducer {
    fn backend(&self) -> &'static QueueBackend {
        &ELSEWHERE
    }

    async fn enqueue(
        &self,
        _queue: &QueueName,
        _envelopes: Vec<Envelope>,
        _options: &PushOptions,
    ) -> Result<(), QueueError> {
        Ok(())
    }
}

/// A second queue backend's binding, declared the way the port's contract asks.
struct ElsewhereQueueModule;

impl Module for ElsewhereQueueModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_declared_factory::<Arc<dyn JobProducer>, _, _>(
            nest_rs_queue::BACKEND_REMEDY,
            |_| async { Ok(Arc::new(ElsewhereProducer) as Arc<dyn JobProducer>) },
        )
    }
}

#[module(imports = [
    RedisModule::for_root(RedisConfig::default()),
    RedisQueueModule,
    ElsewhereQueueModule,
])]
struct TwoBackendsModule;

/// Without the declaration, whichever binding `imports` listed last would serve
/// every push in silence — and a feature injecting `Arc<dyn JobProducer>` never
/// names the backend, so nothing else would ever say which one it got.
#[tokio::test]
async fn a_second_queue_backend_beside_redis_fails_the_boot_naming_the_remedy() {
    let Err(error) = App::builder().module::<TwoBackendsModule>().build().await else {
        panic!("two queue backends must not boot");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains(nest_rs_queue::BACKEND_REMEDY),
        "the boot error carries the port's remedy: {rendered}",
    );
}
