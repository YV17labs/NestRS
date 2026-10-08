//! The cross-process [`RedisThrottler`] store against a real Redis: **one
//! budget shared across `RedisThrottler` instances**, i.e. across app replicas.

use std::time::{Duration, Instant};

use nest_rs_redis::RedisThrottler;
use nest_rs_throttler::{Throttle, ThrottlerStore};

use crate::{connect, unique_key};

/// Where the store counts `subject`'s window: `nestrs:throttler:buckets:<subject>`
/// — the throttler's concern, then the structure the port calls a bucket.
/// Spelled here rather than read from the crate, so the layout an operator
/// scans for is pinned by a test instead of trusted.
fn bucket(subject: &str) -> String {
    format!("nestrs:throttler:buckets:{subject}")
}

#[tokio::test]
async fn a_hit_counts_in_the_bucket_named_for_its_subject() {
    let limit = Throttle::new(5, Duration::from_secs(30));
    let conn = connect().await;
    let store = RedisThrottler::new(conn.clone());
    let subject = unique_key("layout");

    for _ in 0..2 {
        assert!(store.hit(&subject, limit).await.allowed);
    }

    let mut reading = conn.clone();
    let count: i64 = redis::cmd("GET")
        .arg(bucket(&subject))
        .query_async(&mut reading)
        .await
        .expect("the bucket holds the window's count");
    assert_eq!(count, 2, "both hits counted in the subject's bucket");
    let ttl_ms: i64 = redis::cmd("PTTL")
        .arg(bucket(&subject))
        .query_async(&mut reading)
        .await
        .expect("PTTL the bucket");
    assert!(
        ttl_ms > 0 && ttl_ms <= 30_000,
        "the window is the bucket's expiry, got {ttl_ms}ms",
    );
}

#[tokio::test]
async fn allows_up_to_the_limit_then_denies_with_a_retry_after() {
    // A generous window so the counter can't roll over mid-test.
    let limit = Throttle::new(3, Duration::from_secs(30));
    let store = RedisThrottler::new(connect().await);
    let key = unique_key("cap");

    for n in 1..=3 {
        let decision = store.hit(&key, limit).await;
        assert!(decision.allowed, "hit {n} of 3 must be allowed");
        assert_eq!(decision.retry_after, Duration::ZERO);
    }

    let denied = store.hit(&key, limit).await;
    assert!(!denied.allowed, "the 4th hit must be denied");
    assert!(
        denied.retry_after > Duration::ZERO && denied.retry_after <= limit.window(),
        "Retry-After must be the true remaining window, got {:?}",
        denied.retry_after,
    );
}

/// Each window is full, ending in a few milliseconds set by hand: a denial longer
/// than those came from the ending window. Many rounds, so some hit lands in a last millisecond.
#[tokio::test]
async fn a_hit_in_a_windows_last_millisecond_opens_the_next() {
    let limit = Throttle::new(1, Duration::from_secs(30));
    let ending = Duration::from_millis(3);
    let conn = connect().await;
    let store = RedisThrottler::new(conn.clone());
    for round in 0..20 {
        let subject = unique_key("last-millisecond");
        let _: () = redis::cmd("SET")
            .arg(bucket(&subject))
            .arg(1)
            .arg("PX")
            .arg(ending.as_millis() as u64)
            .query_async(&mut conn.clone())
            .await
            .expect("a full window ending soon");
        let set = Instant::now();
        loop {
            let decision = store.hit(&subject, limit).await;
            if decision.allowed {
                break;
            }
            assert!(
                decision.retry_after <= ending,
                "round {round}: a window ending in {ending:?} denied for {:?}",
                decision.retry_after,
            );
            assert!(
                set.elapsed() < Duration::from_secs(1),
                "round {round}: a window ending in {ending:?} still denies a second later",
            );
        }
    }
}

#[tokio::test]
async fn distinct_keys_have_independent_budgets() {
    let limit = Throttle::new(1, Duration::from_secs(30));
    let store = RedisThrottler::new(connect().await);
    let key_a = unique_key("indep-a");
    let key_b = unique_key("indep-b");

    assert!(
        store.hit(&key_a, limit).await.allowed,
        "a: first hit allowed"
    );
    assert!(
        store.hit(&key_b, limit).await.allowed,
        "b: first hit allowed"
    );
    assert!(
        !store.hit(&key_a, limit).await.allowed,
        "a: second hit denied — b's hit must not have spent a's budget",
    );
}

#[tokio::test]
async fn the_budget_is_shared_across_store_instances() {
    let limit = Throttle::new(2, Duration::from_secs(30));
    let replica_a = RedisThrottler::new(connect().await);
    let replica_b = RedisThrottler::new(connect().await);
    let key = unique_key("shared");

    assert!(
        replica_a.hit(&key, limit).await.allowed,
        "replica a: count 1"
    );
    assert!(
        replica_b.hit(&key, limit).await.allowed,
        "replica b: count 2"
    );
    assert!(
        !replica_a.hit(&key, limit).await.allowed,
        "replica a: count 3 must be denied — the two replicas share one budget",
    );
}

/// The three shapes booted together; the vendor binding is listed **first** on
/// purpose, since its factories declare what they run after.
mod module {
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::{App, module};
    use nest_rs_redis::{RedisModule, RedisThrottlerModule};
    use nest_rs_throttler::{
        Throttle, ThrottlerConfig, ThrottlerGuard, ThrottlerModule, ThrottlerSetup, ThrottlerStore,
    };

    use crate::{redis_config, unique_key};

    /// The key the suite's apps count under — a fixture, never a secret.
    const PSEUDONYM_KEY: &str = "nest-rs-redis e2e throttler pseudonym key";

    /// A limit no default could produce, so the assertion below can only pass by
    /// way of this call.
    fn pinned_policy() -> ThrottlerSetup {
        ThrottlerModule::for_root(ThrottlerConfig {
            limit: Some(2),
            window_secs: Some(30),
            pseudonym_key: Some(PSEUDONYM_KEY.to_owned()),
        })
    }

    #[module(imports = [
        RedisThrottlerModule,
        pinned_policy(),
        RedisModule::for_root(redis_config()),
    ])]
    struct RedisThrottlerHost;

    async fn boot() -> App {
        App::builder()
            .module::<RedisThrottlerHost>()
            .build()
            .await
            .expect("the Redis-backed throttler boots against the dev container")
    }

    fn store_of(app: &App) -> Arc<dyn ThrottlerStore> {
        app.container()
            .get::<Arc<dyn ThrottlerStore>>()
            .map(|store| (*store).clone())
            .expect("the binding supersedes the port's default store")
    }

    #[tokio::test]
    async fn the_policy_reaches_the_guard_and_the_store_is_one_two_apps_share() {
        let first = boot().await;
        assert!(
            first.container().get::<ThrottlerGuard>().is_some(),
            "ThrottlerModule::for_root registers the guard as global infrastructure",
        );
        let limit = Throttle::new(2, Duration::from_secs(30));
        let key = unique_key("for-root-shared");

        // A second boot is a second app instance: an in-memory store would hand
        // it a fresh budget.
        let second = boot().await;
        let (a, b) = (store_of(&first), store_of(&second));

        assert!(a.hit(&key, limit).await.allowed, "app one, hit 1 of 2");
        assert!(b.hit(&key, limit).await.allowed, "app two, hit 2 of 2");
        assert!(
            !a.hit(&key, limit).await.allowed,
            "the third hit is denied — both apps counted against one budget",
        );
    }

    #[module(imports = [
        RedisThrottlerModule,
        ThrottlerModule::for_root(None),
        RedisModule::for_root(redis_config()),
    ])]
    struct KeylessThrottlerHost;

    #[tokio::test]
    async fn the_redis_store_without_a_pseudonym_key_fails_the_boot() {
        let Err(refused) = App::builder()
            .module::<KeylessThrottlerHost>()
            .build()
            .await
        else {
            panic!("a Redis-backed throttler must not boot without a pseudonym key");
        };
        let said = format!("{refused:#}");
        assert!(
            said.contains(&nest_rs_config::var_name("throttler", "PSEUDONYM_KEY"))
                && said.contains("RedisThrottler"),
            "{said}"
        );
    }

    #[module(imports = [
        RedisThrottlerModule,
        pinned_policy(),
        RedisModule::for_root(nest_rs_redis::RedisConfig {
            connect_timeout: nest_rs_throttler::HIT_TIMEOUT,
            ..redis_config()
        }),
    ])]
    struct PatientThrottlerHost;

    #[tokio::test]
    async fn a_budget_at_the_guards_net_fails_the_boot() {
        let Err(refused) = App::builder()
            .module::<PatientThrottlerHost>()
            .build()
            .await
        else {
            panic!("a budget at the guard's net must not boot");
        };
        let refused = crate::budget_past_net(refused);
        assert_eq!(
            (refused.resource, refused.port, refused.net),
            (
                "the Redis connection",
                "the rate limiter",
                nest_rs_throttler::HIT_TIMEOUT
            )
        );
    }
}

/// The error is a `WRONGTYPE`, not a dead port: `RedisConnection::connect`
/// refuses an unreachable endpoint up front, so a store always has a connection.
#[tokio::test]
async fn a_store_that_cannot_answer_denies_rather_than_letting_the_caller_through() {
    let logs = nest_rs_testing::LogCapture::install();
    let limit = Throttle::new(3, Duration::from_secs(30));
    let conn = connect().await;
    let key = unique_key("unavailable");

    // Make the window key a hash, so the fixed-window script's `INCR` fails.
    let namespaced = bucket(&key);
    let mut seeding = conn.clone();
    redis::cmd("HSET")
        .arg(&namespaced)
        .arg("field")
        .arg("value")
        .query_async::<()>(&mut seeding)
        .await
        .expect("seed a key the window script cannot count");
    // With an expiry: the window script never reaches its `PEXPIRE` on this key,
    // and a failed run would leave an immortal hash in the shared Redis.
    redis::cmd("EXPIRE")
        .arg(&namespaced)
        .arg(300)
        .query_async::<()>(&mut seeding)
        .await
        .expect("bound the probe key's lifetime");

    let store = RedisThrottler::new(conn);
    let decision = store.hit(&key, limit).await;

    assert!(
        !decision.allowed,
        "a store that cannot answer must deny — failing open here is a rate \
         limit that disappears exactly when the backend is in trouble",
    );
    assert!(
        decision.retry_after > Duration::ZERO,
        "…and tell the caller when to come back, rather than refusing with no \
         way forward: {:?}",
        decision.retry_after,
    );

    let event = logs.expect_one(
        nest_rs_throttler::TARGET,
        "redis throttler unavailable; denying (fail-closed)",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("key").as_deref(),
        Some(key.as_str()),
        "the event names the client whose request was refused, which is what \
         separates an outage from a caller who really is over budget: {:?}",
        event.fields,
    );
    assert!(
        event.field("error").is_some(),
        "…and what the store said, got {:?}",
        event.fields,
    );
}

/// The ACL user this test creates from the page, and removes — named for this
/// run, so no earlier run's denial in `ACL LOG` is read as its own.
const CONFINED_USER: &str = "nestrs-e2e-throttler";

/// Redis checks a script's own `INCR`, `PTTL` and `PEXPIRE` against the caller,
/// not only the `EVALSHA` that carries them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_created_as_the_rate_limiting_page_says_counts_and_denies() {
    let user = crate::acl_user(CONFINED_USER);
    let config = crate::documented_user("rate-limiting/index.mdx", "rate limiter", &user, 0).await;
    crate::assert_may_load_a_script(&config).await;
    let conn = nest_rs_redis::RedisConnection::connect(&config)
        .await
        .expect("the page's user passes the boot's proof");
    let store = RedisThrottler::new(conn);
    let limit = Throttle::new(2, Duration::from_secs(30));
    let subject = unique_key("acl");

    let decisions = [
        store.hit(&subject, limit).await,
        store.hit(&subject, limit).await,
        store.hit(&subject, limit).await,
    ];
    crate::forget_user(&user).await;

    let allowed = decisions.each_ref().map(|decision| decision.allowed);
    assert_eq!(
        allowed,
        [true, true, false],
        "two hits counted through the ACL, the third denied by the count"
    );
    assert!(
        decisions[2].retry_after > Duration::ZERO && decisions[2].retry_after <= limit.window(),
        "denied with the window's remainder: {:?}",
        decisions[2].retry_after
    );
    crate::assert_redis_denied_nothing_but(&user, &[]).await;
}
