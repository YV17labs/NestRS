//! `RedisQueueModule`'s composition, in process: the portable producer and the
//! consumer are *declarations*, so a second queue backend imported beside it
//! fails the boot naming the remedy — before any factory runs, so before Redis
//! is dialled.

use std::sync::Arc;

use nest_rs_core::{App, ContainerBuilder, Imported, Module, module};
use nest_rs_queue::{
    Ask, BoundConsumer, Capabilities, Disposition, Envelope, JobConsumer, JobProducer, LeaseHold,
    Prepared, ProcessMethod, PushOptions, QueueBackend, QueueError, QueueName, Received,
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
    fn register(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
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

/// Another backend's consumer — what a second queue binding would declare.
struct ElsewhereConsumer;

impl JobConsumer for ElsewhereConsumer {
    type Lease = ();

    fn backend(&self) -> &'static QueueBackend {
        &ELSEWHERE
    }

    async fn prepare(&self, _methods: &[&'static ProcessMethod]) -> Result<Prepared, QueueError> {
        Ok(Prepared::new(std::time::Duration::from_secs(1)))
    }

    async fn receive(
        &self,
        _method: &'static ProcessMethod,
        _ask: Ask,
    ) -> Result<Received<()>, QueueError> {
        Ok(Received::new(Vec::new()))
    }

    async fn renew(
        &self,
        _method: &'static ProcessMethod,
        leases: &[&()],
    ) -> Result<Vec<LeaseHold>, QueueError> {
        Ok(vec![LeaseHold::Held; leases.len()])
    }

    async fn settle(
        &self,
        _method: &'static ProcessMethod,
        _lease: &(),
        _disposition: Disposition<'_>,
    ) -> Result<LeaseHold, QueueError> {
        Ok(LeaseHold::Held)
    }
}

/// A second queue backend's consumer binding, declared the way the port's
/// contract asks.
struct ElsewhereConsumerModule;

impl Module for ElsewhereConsumerModule {
    fn register(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Imported<Self>) -> ContainerBuilder {
        builder.provide_declared_factory::<BoundConsumer, _, _>(
            nest_rs_queue::BACKEND_REMEDY,
            |_| async { Ok(BoundConsumer::new(ElsewhereConsumer)) },
        )
    }
}

#[module(imports = [
    RedisModule::for_root(RedisConfig::default()),
    RedisQueueModule,
    ElsewhereConsumerModule,
])]
struct TwoConsumersModule;

/// The consumer is declared too: without it, whichever binding `imports`
/// listed last would run every job in silence, and the producer and the
/// consumer could each be another backend's.
#[tokio::test]
async fn a_second_queue_consumer_beside_redis_fails_the_boot_naming_the_remedy() {
    let Err(error) = App::builder().module::<TwoConsumersModule>().build().await else {
        panic!("two queue consumers must not boot");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains(nest_rs_queue::BACKEND_REMEDY),
        "the boot error carries the port's remedy: {rendered}",
    );
}

#[tokio::test]
async fn the_binding_registered_without_its_collect_fails_the_boot_naming_its_config() {
    assert_eq!(
        crate::registered_alone::<RedisQueueModule>().await,
        std::any::type_name::<nest_rs_redis::RedisQueueConfig>()
    );
}
