//! `RedisScheduleModule`'s composition, in process: what the boot refuses before
//! any factory runs, so before Redis is dialled.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, Collecting, ContainerBuilder, Module, Registering, module};
use nest_rs_redis::{RedisConfig, RedisModule, RedisScheduleModule};
use nest_rs_schedule::{Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError};

/// A Redis nothing listens on: were a factory to run, the boot would fail on
/// the dial at once rather than reach whatever Redis the machine holds.
fn nowhere() -> RedisConfig {
    RedisConfig {
        url: "rediss://127.0.0.1:1/".to_owned(),
        connect_timeout: Duration::from_millis(200),
        ..RedisConfig::default()
    }
}

/// Another backend's lock — what a second occurrence-lock binding would declare.
struct ElsewhereLock;

#[async_trait::async_trait]
impl OccurrenceLock for ElsewhereLock {
    async fn claim(
        &self,
        _occurrence: &Occurrence,
    ) -> Result<OccurrenceClaim, OccurrenceLockError> {
        Ok(OccurrenceClaim::Claimed)
    }

    async fn claimed(&self, _token: &str) -> Result<bool, OccurrenceLockError> {
        Ok(false)
    }
}

/// A second lock backend's binding, declared the way the port's contract asks.
struct ElsewhereScheduleModule;

impl Module for ElsewhereScheduleModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_declared_factory::<Arc<dyn OccurrenceLock>, _, _>(
            nest_rs_schedule::BACKEND_REMEDY,
            |_| async { Ok(Arc::new(ElsewhereLock) as Arc<dyn OccurrenceLock>) },
        )
    }
}

#[module(imports = [
    RedisModule::for_root(nowhere()),
    RedisScheduleModule,
    ElsewhereScheduleModule,
])]
struct TwoLocksModule;

#[module(imports = [RedisScheduleModule])]
struct NoConnectionModule;

#[tokio::test]
async fn a_second_occurrence_lock_beside_redis_fails_the_boot_naming_the_remedy() {
    let Err(error) = App::builder().module::<TwoLocksModule>().build().await else {
        panic!("two occurrence-lock backends must not boot");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains(nest_rs_schedule::BACKEND_REMEDY),
        "the boot error carries the port's remedy: {rendered}",
    );
}

#[tokio::test]
async fn the_binding_without_its_connection_fails_the_boot_naming_the_import() {
    let Err(error) = App::builder().module::<NoConnectionModule>().build().await else {
        panic!("a lock binding with no connection to claim over must not boot");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("RedisModule::for_root(None)"),
        "the boot error names the import that opens the connection: {rendered}",
    );
}

#[tokio::test]
async fn the_binding_registered_without_its_collect_fails_the_boot_naming_its_lock() {
    assert_eq!(
        crate::registered_alone::<RedisScheduleModule>().await,
        std::any::type_name::<Arc<dyn OccurrenceLock>>()
    );
}
