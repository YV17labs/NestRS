//! `RedisQueueModule`'s composition, in process: a second queue backend imported
//! beside it fails the boot before any factory runs, so before Redis is dialled.

use std::sync::Arc;

use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering, module};
use nest_rs_queue::{
    Ask, BoundConsumer, Capabilities, Disposition, Envelope, JobConsumer, JobProducer, Jobs,
    LeaseHold, Prepared, ProcessMethod, PushOptions, QueueBackend, QueueError, QueueName, Received,
};
use nest_rs_redis::{RedisConfig, RedisModule, RedisQueueModule, RedisQueueProducer};

use crate::module::Cache;

static ELSEWHERE: QueueBackend = QueueBackend::new("elsewhere", Capabilities::NONE);

/// Another backend's producer — what a second queue binding would declare.
struct ElsewhereProducer;

#[nest_rs_core::async_trait]
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
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_declared_factory::<Jobs, _, _>(nest_rs_queue::BACKEND_REMEDY, |_| async {
            Ok(Jobs::new(Arc::new(ElsewhereProducer)))
        })
    }
}

#[module(imports = [
    RedisModule::for_root(RedisConfig::default()),
    RedisQueueModule,
    ElsewhereQueueModule,
])]
struct TwoBackendsModule;

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
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
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

/// A queue on a named instance reads its config under that instance.
#[tokio::test]
async fn a_binding_of_a_named_instance_reads_its_config_under_that_instance() {
    assert_eq!(
        crate::registered_alone::<RedisQueueModule<Cache>>().await,
        std::any::type_name::<nest_rs_core::Instanced<Cache, nest_rs_redis::RedisQueueConfig>>()
    );
}

#[module(imports = [
    RedisModule::for_root(RedisConfig::default()),
    RedisModule::for_root((Cache, RedisConfig::default())),
    RedisQueueModule,
    RedisQueueModule::<Cache>,
])]
struct QueueOnTwoInstancesModule;

/// One binding per port, whichever instances the bindings run on.
#[tokio::test]
async fn a_queue_on_each_of_two_instances_fails_the_boot_naming_the_remedy() {
    let Err(error) = App::builder()
        .module::<QueueOnTwoInstancesModule>()
        .build()
        .await
    else {
        panic!("two queue bindings must not boot, whatever their instances");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("contested declaration")
            && rendered.contains(nest_rs_queue::BACKEND_REMEDY),
        "{rendered}",
    );
}

#[expect(
    dead_code,
    reason = "the producer is declared for the container to resolve, never read"
)]
#[nest_rs_core::injectable]
struct PushesOnTheDefault {
    #[inject]
    producer: Arc<RedisQueueProducer>,
}

#[module(providers = [PushesOnTheDefault])]
struct NoQueueModule;

#[expect(
    dead_code,
    reason = "the producer is declared for the container to resolve, never read"
)]
#[nest_rs_core::injectable]
struct PushesOnTheCache {
    #[inject]
    producer: Arc<RedisQueueProducer<Cache>>,
}

#[module(providers = [PushesOnTheCache])]
struct NoQueueOnTheCacheModule;

/// What `M`'s boot says of the producer no binding registers.
async fn unmet_producer<M: Module + 'static>() -> String {
    let Err(refused) = App::builder().module::<M>().build().await else {
        panic!("no binding registers the producer");
    };
    refused
        .downcast::<nest_rs_core::MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"))
        .to_string()
}

/// The producer is the binding's, on each instance: the refusal names the
/// binding to import, and the port's handle a feature pushes through.
#[tokio::test]
async fn a_provider_injecting_a_producer_no_binding_registers_is_told_the_binding() {
    let said = unmet_producer::<NoQueueModule>().await;
    assert!(
        said.contains("depends on `RedisQueueProducer`")
            && said.ends_with(
                "which no module this app imports provides. Import \
                 `nest_rs::redis::RedisQueueModule`, which binds the queue port on the default \
                 instance; a feature pushes jobs through the port's \
                 `Arc<nest_rs::queue::Jobs>`."
            ),
        "{said}"
    );
    let said = unmet_producer::<NoQueueOnTheCacheModule>().await;
    assert!(
        said.contains("depends on `RedisQueueProducer<Cache>`")
            && said.ends_with(
                "which no module this app imports provides. Import \
                 `nest_rs::redis::RedisQueueModule::<Cache>`, which binds the queue port on the \
                 `cache` instance; a feature pushes jobs through the port's \
                 `Arc<nest_rs::queue::Jobs>`."
            ),
        "{said}"
    );
}
