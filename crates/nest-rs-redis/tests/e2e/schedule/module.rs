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

#[tokio::test]
async fn a_budget_at_the_schedulers_net_fails_the_boot() {
    let Err(refused) = App::builder().module::<PatientLockModule>().build().await else {
        panic!("a budget at the scheduler's net must not boot");
    };
    let refused = crate::budget_past_net(refused);
    assert_eq!(
        (refused.resource, refused.port, refused.net),
        (
            "the Redis connection",
            "the scheduler",
            nest_rs_schedule::LOCK_TIMEOUT
        )
    );
}
