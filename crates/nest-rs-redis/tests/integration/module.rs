//! `RedisModule::for_root` opening a named instance beside the default one, in
//! process: two connections to two scripted servers, a named instance's budget
//! held under each binding's net, a queue's config pinned on it, its refusals
//! naming its own variables, its namespace refused when a linked config holds
//! it, and its variables claimed by the read that resolved them.

use std::sync::OnceLock;
use std::time::Duration;

use nest_rs_config::{Config, ConfigError, ConfigService, config};
use nest_rs_core::{App, Instanced, instance, module};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisError, RedisModule, RedisQueueConfig, RedisQueueModule,
    RedisScheduleModule, RedisThrottlerModule,
};

use crate::connection::DemotedRedis;
use crate::harness::tls::trusting_the_test_authority;

/// The suite's one named instance: two markers reading `cache` in one binary
/// would be refused as twins.
#[instance]
pub(crate) struct Cache;

/// The cache instance's server, set by each test booting a module that pins
/// [`cache_config`] — [`TwoInstances`], `health`'s two probed instances:
/// nextest runs each test in a process of its own.
pub(crate) static CACHE_URL: OnceLock<String> = OnceLock::new();

pub(crate) fn cache_config() -> RedisConfig {
    RedisConfig {
        url: CACHE_URL.get().cloned().unwrap_or_default(),
        connect_timeout: Duration::from_secs(2),
        tls: trusting_the_test_authority(),
    }
}

/// The default instance, seeded by the test — the suite's own
/// `<PREFIX>_REDIS__URL` would outrank a pin — beside the cache pinned here,
/// which no variable of the suite's reaches.
#[module(imports = [RedisModule::for_root(None), RedisModule::for_root((Cache, cache_config()))])]
struct TwoInstances;

/// Two instances open two connections, each to its own server: the primary
/// behind the default one is demoted, and the cache's still takes a write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_instances_open_two_connections_each_to_its_own_server() {
    let primary = DemotedRedis::start().await;
    let cache = DemotedRedis::start().await;
    CACHE_URL.set(cache.url()).expect("set once");
    let app = App::builder()
        .provide(RedisConfig {
            url: primary.url(),
            connect_timeout: Duration::from_secs(2),
            tls: trusting_the_test_authority(),
        })
        .module::<TwoInstances>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("both instances boot: {refused:#}"));
    let container = app.container();
    let mut primary_conn = RedisConnection::clone(
        &container
            .get::<RedisConnection>()
            .expect("the default instance's connection"),
    );
    let mut cache_conn = RedisConnection::clone(
        &container
            .get::<RedisConnection<Cache>>()
            .expect("the cache's connection"),
    );
    assert!(
        container.get::<Instanced<Cache, RedisConfig>>().is_some(),
        "the cache's config is held under its instance"
    );

    primary.demote();
    let write = || redis::cmd("SET").arg("nestrs-instance").arg("1").clone();
    let refused = write()
        .query_async::<()>(&mut primary_conn)
        .await
        .expect_err("the default instance reaches the demoted server");
    assert_eq!(refused.code(), Some("READONLY"), "{refused}");
    write()
        .query_async::<()>(&mut cache_conn)
        .await
        .expect("the cache reaches a server of its own");
}

/// The cache's budget pinned at the rate limiter's net, on a server that
/// answers: the boot holds the cache's budget, not the default's, under it.
#[module(imports = [RedisModule::for_root((Cache, None)), RedisThrottlerModule::<Cache>])]
struct PatientCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_instance_s_budget_at_its_binding_s_net_is_refused_naming_its_variable() {
    let cache = DemotedRedis::start().await;
    let Err(refused) = App::builder()
        .provide(Instanced::<Cache, RedisConfig>::from(RedisConfig {
            url: cache.url(),
            connect_timeout: nest_rs_throttler::HIT_TIMEOUT,
            tls: trusting_the_test_authority(),
        }))
        .module::<PatientCache>()
        .build()
        .await
    else {
        panic!("a budget at the rate limiter's net must not boot");
    };
    let refused = refused
        .downcast::<nest_rs_core::BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.port, refused.net),
        (
            "the Redis connection",
            "the rate limiter",
            nest_rs_throttler::HIT_TIMEOUT
        )
    );
    assert!(
        refused.setting.contains(&nest_rs_config::var_name(
            "redis__cache",
            "CONNECT_TIMEOUT_SECS"
        )),
        "{refused}"
    );
}

/// The cache's budget pinned at the scheduler's net.
#[module(imports = [RedisModule::for_root((Cache, None)), RedisScheduleModule::<Cache>])]
struct LockingCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_instance_s_budget_at_the_scheduler_s_net_is_refused_naming_its_variable() {
    let cache = DemotedRedis::start().await;
    let Err(refused) = App::builder()
        .provide(Instanced::<Cache, RedisConfig>::from(RedisConfig {
            url: cache.url(),
            connect_timeout: nest_rs_schedule::LOCK_TIMEOUT,
            tls: trusting_the_test_authority(),
        }))
        .module::<LockingCache>()
        .build()
        .await
    else {
        panic!("a budget at the scheduler's net must not boot");
    };
    let refused = refused
        .downcast::<nest_rs_core::BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.port, refused.net),
        (
            "the Redis connection",
            "the scheduler",
            nest_rs_schedule::LOCK_TIMEOUT
        )
    );
    assert!(
        refused.setting.contains(&nest_rs_config::var_name(
            "redis__cache",
            "CONNECT_TIMEOUT_SECS"
        )),
        "{refused}"
    );
}

/// The lease a queue on the cache is pinned with, unlike the default's.
const CACHE_LEASE: Duration = Duration::from_secs(45);

/// A queue on the cache, its config pinned on the binding.
#[module(imports = [
    RedisModule::for_root((Cache, None)),
    RedisQueueModule::for_root((Cache, RedisQueueConfig { lease: CACHE_LEASE })),
])]
struct QueueOnTheCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queue_pinned_on_a_named_instance_holds_its_config_under_that_instance() {
    assert_ne!(RedisQueueConfig::default().lease, CACHE_LEASE);
    let cache = DemotedRedis::start().await;
    let app = App::builder()
        .provide(Instanced::<Cache, RedisConfig>::from(RedisConfig {
            url: cache.url(),
            connect_timeout: Duration::from_secs(2),
            tls: trusting_the_test_authority(),
        }))
        .module::<QueueOnTheCache>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the queue on the cache boots: {refused:#}"));
    let held = app
        .container()
        .get::<Instanced<Cache, RedisQueueConfig>>()
        .expect("the queue's config is held under the cache");
    assert_eq!(held.lease, CACHE_LEASE);
    assert!(
        app.container().get::<RedisQueueConfig>().is_none(),
        "the default instance has no queue"
    );
}

/// An instance reading `lock`, whose derived namespace the config below holds.
#[instance]
struct Lock;

/// A binding's config declaring `redis__lock` — a word no reserved list can
/// know — so the instance `Lock` would read its variables.
#[config(namespace = "redis__lock")]
#[derive(Clone, Default)]
struct RedisLockConfig {
    holders: u64,
}

impl Config for RedisLockConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            holders: env.parse("HOLDERS")?.unwrap_or(base.holders),
        })
    }
}

#[module(imports = [RedisModule::for_root((Lock, None))])]
struct LockSharingItsNamespace;

#[tokio::test]
async fn an_instance_whose_namespace_a_linked_config_declares_is_refused() {
    let Err(refused) = App::builder()
        .module::<LockSharingItsNamespace>()
        .build()
        .await
    else {
        panic!("an instance sharing its namespace must not boot");
    };
    let Some(ConfigError::SharedInstance {
        instance,
        namespace,
        claims,
    }) = refused
        .chain()
        .find_map(|cause| cause.downcast_ref::<ConfigError>())
    else {
        panic!("not a shared-instance refusal: {refused:#}");
    };
    assert!(instance.ends_with("::Lock"), "{refused:#}");
    assert_eq!(namespace, "redis__lock");
    assert_eq!(claims.len(), 1, "{refused:#}");
    assert!(claims[0].ends_with("::RedisLockConfig"), "{refused:#}");
    RedisLockConfig::load().expect("the config keeps its own namespace");
}

/// The cache pinned at a port nothing listens on.
#[module(imports = [RedisModule::for_root((Cache, RedisConfig {
    url: "rediss://127.0.0.1:1/".to_owned(),
    connect_timeout: Duration::from_millis(300),
    tls: trusting_the_test_authority(),
}))])]
struct UnreachableCache;

#[tokio::test]
async fn an_unreachable_named_instance_fails_the_boot_naming_its_url() {
    let Err(refused) = App::builder().module::<UnreachableCache>().build().await else {
        panic!("a cache nothing serves must not boot");
    };
    let said = format!("{refused:#}");
    assert!(
        refused.chain().any(|cause| matches!(
            cause.downcast_ref::<RedisError>(),
            Some(RedisError::Unreachable { .. })
        )),
        "not the connection's refusal: {said}"
    );
    assert!(
        said.contains(&nest_rs_config::var_name("redis__cache", "URL"))
            && said.contains(&nest_rs_config::var_name(
                "redis__cache",
                "CONNECT_TIMEOUT_SECS"
            ))
            && !said.contains(&nest_rs_config::var_name("redis", "URL")),
        "{said}"
    );
}

#[module(imports = [RedisModule::for_root(None), RedisModule::for_root((Cache, None))])]
struct BothFromTheEnvironment;

/// The cache's variables are the cache's, though the default instance's keys
/// are checked first — its read comes first, and its connection's refusal ends
/// the boot before the cache is read — while a key the default instance does
/// not read is still reported.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn the_unclaimed_report_leaves_a_named_instance_s_variables_alone() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let nowhere = "rediss://127.0.0.1:1/";
    let cache = |key| nest_rs_config::var_name("redis__cache", key);
    figment::Jail::expect_with(|jail| {
        jail.set_env(nest_rs_config::var_name("redis", "URL"), nowhere);
        jail.set_env(
            nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
            "1",
        );
        jail.set_env(cache("URL"), nowhere);
        jail.set_env(cache("CONNECT_TIMEOUT_SECS"), "1");
        jail.set_env(nest_rs_config::var_name("redis", "NOT_A_KEY"), "1");
        let booted = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(App::builder().module::<BothFromTheEnvironment>().build());
        assert!(booted.is_err(), "nothing serves either instance");
        Ok(())
    });

    let reported = |variable: &str| {
        logs.events()
            .into_iter()
            .filter(|event| event.target == nest_rs_config::TARGET)
            .filter(|event| event.field("variable").as_deref() == Some(variable))
            .collect::<Vec<_>>()
    };
    for claimed in [cache("URL"), cache("CONNECT_TIMEOUT_SECS")] {
        assert!(
            reported(&claimed).is_empty(),
            "{claimed} is read: {:#?}",
            reported(&claimed)
        );
    }
    let unread = reported(&nest_rs_config::var_name("redis", "NOT_A_KEY"));
    assert!(
        unread.iter().any(|event| {
            event.message == nest_rs_config::unclaimed::UNREAD_CONFIG_VARIABLE
                && event.field("namespace").as_deref() == Some("redis")
        }),
        "the default instance's keys were checked: {:#?}",
        logs.events()
    );
}
