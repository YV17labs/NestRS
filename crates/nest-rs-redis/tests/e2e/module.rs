//! `RedisModule::for_root` opening a named instance beside the default one on
//! the suite's Valkey: the queue binds the default instance, the rate limiter
//! a `cache` instance on another logical database, and each counts where its
//! instance's URL points.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, instance, module};
use nest_rs_queue::JobProducer;
use nest_rs_redis::{
    RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer, RedisThrottlerModule,
};
use nest_rs_throttler::{Throttle, ThrottlerStore};
use redis::aio::ConnectionLike;

use crate::{
    DB_SELECT_RETRIED, at_default_budget, connect, connect_on, redis_config, redis_config_on,
    unique_key,
};

/// The suite's one named instance: two markers reading `cache` in one binary
/// would be refused as twins.
#[instance]
pub(crate) struct Cache;

/// The suite's Valkey has no logical database to spare, so the cache reuses
/// [`DB_SELECT_RETRIED`], whose test only proves a connection: the one key
/// this test writes there is named for the run, expires with its window, and
/// is deleted before the test ends. No `<PREFIX>_REDIS__CACHE__*` variable of
/// the suite's reaches the pin.
#[module(imports = [
    RedisModule::for_root(at_default_budget(redis_config())),
    RedisQueueModule,
    RedisModule::for_root((Cache, at_default_budget(redis_config_on(DB_SELECT_RETRIED)))),
    RedisThrottlerModule::<Cache>,
])]
struct QueueAndCache;

#[tokio::test]
async fn the_queue_binds_the_default_instance_and_the_rate_limiter_the_cache() {
    let app = App::builder()
        .module::<QueueAndCache>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("both instances boot: {refused:#}"));
    let container = app.container();
    let default = container
        .get::<RedisConnection>()
        .expect("the default instance's connection");
    let cache = container
        .get::<RedisConnection<Cache>>()
        .expect("the cache's connection");
    assert_eq!(cache.get_db(), i64::from(DB_SELECT_RETRIED));
    assert_ne!(default.get_db(), cache.get_db());
    assert!(
        container.get::<RedisQueueProducer>().is_some()
            && container.get::<Arc<dyn JobProducer>>().is_some(),
        "the queue binds the default instance"
    );

    let store = container
        .get::<Arc<dyn ThrottlerStore>>()
        .map(|store| (*store).clone())
        .expect("the rate limiter's store");
    let subject = unique_key("cache-instance");
    assert!(
        store
            .hit(&subject, Throttle::new(5, Duration::from_secs(30)))
            .await
            .expect("counted")
            .allowed
    );
    let bucket = format!("nestrs:throttler:buckets:{subject}");
    let mut on_cache = connect_on(DB_SELECT_RETRIED).await;
    let mut on_default = connect().await;
    let counted: Option<i64> = redis::cmd("GET")
        .arg(&bucket)
        .query_async(&mut on_cache)
        .await
        .expect("GET the cache's bucket");
    let elsewhere: i64 = redis::cmd("EXISTS")
        .arg(&bucket)
        .query_async(&mut on_default)
        .await
        .expect("EXISTS on the default instance's database");
    let _: i64 = redis::cmd("DEL")
        .arg(&bucket)
        .query_async(&mut on_cache)
        .await
        .expect("DEL the bucket");
    assert_eq!(counted, Some(1), "the hit counted on the cache's database");
    assert_eq!(elsewhere, 0, "and nothing on the default instance's");
}
