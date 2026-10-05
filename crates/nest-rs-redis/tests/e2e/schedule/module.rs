//! `RedisScheduleModule` against a live Redis: the budget its connection holds
//! a command to must answer before the scheduler stops waiting on the lock.

use nest_rs_core::{App, module};
use nest_rs_redis::{RedisModule, RedisScheduleModule};

use crate::redis_config;

#[module(imports = [
    RedisModule::for_root(nest_rs_redis::RedisConfig {
        connect_timeout: nest_rs_schedule::LOCK_TIMEOUT,
        ..redis_config()
    }),
    RedisScheduleModule,
])]
struct PatientLockModule;

/// A budget at the scheduler's net would let the scheduler abandon a claim
/// still answering, and skip its occurrence without the cause: the binding
/// refuses it at boot, naming both durations and the variable to lower.
#[tokio::test]
async fn a_budget_at_the_schedulers_net_fails_the_boot() {
    let Err(refused) = App::builder().module::<PatientLockModule>().build().await else {
        panic!("a budget at the scheduler's net must not boot");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains("the scheduler")
            && said.contains(&format!("{:?}", nest_rs_schedule::LOCK_TIMEOUT))
            && said.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
        "{said}"
    );
}
