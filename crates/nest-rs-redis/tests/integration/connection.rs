//! The shared connection's boot, without a Redis: a scripted server that stays
//! busy or refuses a `SELECT`, and the budget's place below the ports' nets.

use std::time::{Duration, Instant};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError};

use crate::harness::connection::{NOT_READY, ScriptedRedis, answer, database_refused_at_once};

/// A Redis that stays not ready spends the budget and fails as one that
/// answered — its last answer as the source, the budget to widen — never as an
/// unreachable one sent to check its URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_that_stays_busy_fails_at_the_budget_naming_its_answer() {
    let proxy = ScriptedRedis::start(None, None).await;
    proxy.answer_with(Some(NOT_READY[0]));
    let budget = Duration::from_millis(1500);
    let started = Instant::now();
    let Err(error) = RedisConnection::connect(&RedisConfig {
        url: proxy.url(),
        connect_timeout: budget,
        ..RedisConfig::default()
    })
    .await
    else {
        panic!("a Redis that never serves must not connect")
    };
    let took = started.elapsed();

    assert!(
        took >= budget && took < budget * 3,
        "at the budget, took {took:?}"
    );
    assert!(matches!(error, RedisError::Unready { .. }), "{error}");
    let rendered = error.to_string();
    assert!(
        rendered.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"))
            && !rendered.contains(&nest_rs_config::var_name("redis", "URL")),
        "the budget is what to widen, not the URL to check: {rendered}"
    );
    assert!(
        answer(&error).contains("busy running a script"),
        "the source is Redis's last answer: {}",
        answer(&error)
    );
}

/// config-1r2: a server in cluster mode answers a `SELECT` with an `ERR` the
/// client reports without its code, so it was retried for the whole budget. It
/// fails at once, naming the index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_serves_database_zero_alone_fails_the_boot_at_once_naming_the_index() {
    let proxy = ScriptedRedis::start(None, None).await;
    proxy.answer_with(Some("ERR SELECT is not allowed in cluster mode"));
    let error =
        database_refused_at_once(format!("{}2", proxy.url()), 2, "a server in cluster mode").await;
    assert!(
        answer(&error).contains("not allowed in cluster mode"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
}

/// The default budget sits below the queue port's net and the throttler
/// guard's, with the room each net's documentation argues — twice the budget,
/// for a deployment that raised it — read from the constants that set them
/// rather than retyped.
#[test]
fn the_connection_budget_answers_before_the_queue_port_and_the_throttler_give_up() {
    let budget = RedisConfig::default().connect_timeout;
    for (net, what) in [
        (nest_rs_queue::BACKEND_TIMEOUT, "the queue port's net"),
        (nest_rs_throttler::HIT_TIMEOUT, "the throttler guard's net"),
    ] {
        assert!(
            budget < net,
            "the connection budget ({budget:?}) must answer before {what} ({net:?})"
        );
        assert!(
            budget * 2 <= net,
            "{what} ({net:?}) leaves room for a budget raised to twice its default ({budget:?})"
        );
    }
}
