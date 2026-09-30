//! `RedisScheduleModule`'s composition, in process: the occurrence lock it binds
//! is a *declaration*, so a second lock backend imported beside it fails the
//! boot naming the port's remedy — before any factory runs, so before Redis is
//! dialled — and the binding imported without the connection it claims over
//! fails the boot naming the import that opens it.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, ContainerBuilder, Module, module};
use nest_rs_redis::{RedisConfig, RedisModule, RedisScheduleModule};
use nest_rs_schedule::{Occurrence, OccurrenceClaim, OccurrenceLock, OccurrenceLockError};

/// A Redis nothing listens on: were a factory to run, the boot would fail on
/// the dial at once rather than reach whatever Redis the machine holds.
fn nowhere() -> RedisConfig {
    RedisConfig {
        url: "redis://127.0.0.1:1/".to_owned(),
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

    async fn renew(&self, _occurrence: &Occurrence) -> Result<bool, OccurrenceLockError> {
        Ok(true)
    }

    async fn release(&self, _occurrence: &Occurrence) -> Result<(), OccurrenceLockError> {
        Ok(())
    }
}

/// A second lock backend's binding, declared the way the port's contract asks.
struct ElsewhereScheduleModule;

impl Module for ElsewhereScheduleModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder) -> ContainerBuilder {
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

/// Without the declaration, whichever binding `imports` listed last would claim
/// every occurrence in silence — and two replicas importing the two in different
/// orders would claim in two stores, so each would fire every occurrence.
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

/// The binding claims over the connection `RedisModule::for_root` opens, and
/// says so when it is imported alone.
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
