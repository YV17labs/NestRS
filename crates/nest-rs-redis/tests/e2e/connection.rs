//! The shared connection against a live Redis — the branches a local listener
//! cannot reach, because they need a server that answers: one that refuses
//! (credentials, a database index, an ACL), one that holds a command, one that
//! vanishes behind a network still accepting the dial, and one that drops every
//! connection the app holds.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError, RedisThrottler};
use nest_rs_throttler::{Throttle, ThrottlerStore};

/// A refusal is one connection attempt, never a budget spent retrying one.
const AT_ONCE: Duration = Duration::from_secs(3);

/// A budget far above [`AT_ONCE`], so a refusal that retried would show.
fn config(url: String) -> RedisConfig {
    RedisConfig {
        url,
        connect_timeout: Duration::from_secs(10),
        ..RedisConfig::default()
    }
}

/// Connect to `url`, which Redis refuses, and return the refusal once it is
/// shown to have come at once, as a refusal, naming the variable to fix.
async fn refused_at_once(url: String, case: &str) -> RedisError {
    let started = Instant::now();
    let Err(error) = RedisConnection::connect(&config(url)).await else {
        panic!("{case} must not connect")
    };
    let took = started.elapsed();
    assert!(
        took < AT_ONCE,
        "{case}: a refusal spends none of the budget, took {took:?}"
    );
    assert!(
        matches!(error, RedisError::Refused { .. }),
        "{case}: {error}"
    );
    assert!(
        error
            .to_string()
            .contains(&nest_rs_config::var_name("redis", "URL")),
        "{case}: the error names the variable to fix: {error}",
    );
    error
}

/// What Redis answered, which the refusal carries as its source.
fn answer(error: &RedisError) -> String {
    std::error::Error::source(error)
        .map(ToString::to_string)
        .unwrap_or_default()
}

/// Credentials Redis refuses are not an outage. Retried as one, the boot spent
/// its whole budget and then told the operator to check the network and widen
/// the timeout.
#[tokio::test]
async fn credentials_redis_refuses_fail_the_boot_at_once_naming_them() {
    let url = crate::redis_url().replacen("://", "://:nestrs-e2e-wrong-secret@", 1);
    let error = refused_at_once(url, "a password Redis does not accept").await;
    assert!(
        answer(&error).contains("authentication failed"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
    assert!(
        !format!("{error} {}", answer(&error)).contains("nestrs-e2e-wrong-secret"),
        "and neither shows the password: {error}",
    );
}

/// A database index Redis does not have is refused on every attempt — the
/// first index past the count the server is configured with, so the test holds
/// whatever that count is.
#[tokio::test]
async fn a_database_index_redis_does_not_have_fails_the_boot_at_once() {
    let databases: Vec<String> = redis::cmd("CONFIG")
        .arg("GET")
        .arg("databases")
        .query_async(&mut crate::connect().await)
        .await
        .expect("CONFIG GET databases");
    let count: u8 = databases
        .get(1)
        .and_then(|count| count.parse().ok())
        .expect("a dev Redis keeps fewer than 256 databases");

    let error = refused_at_once(crate::redis_url_on(count), "a database index out of range").await;
    assert!(
        answer(&error).contains("switch database"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
}

/// An ACL denying the boot's proof is refused on every attempt: the user
/// authenticates, and may not run the `PING` that proves the connection.
#[tokio::test]
async fn an_acl_denying_the_proof_fails_the_boot_at_once() {
    const SECRET: &str = "nestrs-e2e-acl-secret";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let user = format!("nestrs-e2e-no-ping-{}-{nanos}", std::process::id());
    let mut admin = crate::connect().await;
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&user)
        .arg("on")
        .arg(format!(">{SECRET}"))
        .arg("+@all")
        .arg("-ping")
        .query_async::<()>(&mut admin)
        .await
        .expect("ACL SETUSER");

    let url = crate::redis_url().replacen("://", &format!("://{user}:{SECRET}@"), 1);
    let started = Instant::now();
    let outcome = RedisConnection::connect(&config(url)).await;
    let took = started.elapsed();
    redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&user)
        .query_async::<i64>(&mut admin)
        .await
        .expect("ACL DELUSER");

    let Err(error) = outcome else {
        panic!("a user the ACL denies the proof must not connect")
    };
    assert!(
        took < AT_ONCE,
        "a refusal spends none of the budget, took {took:?}"
    );
    assert!(matches!(error, RedisError::Refused { .. }), "{error}");
    assert!(
        answer(&error).contains("NOPERM"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
    assert!(
        !format!("{error} {}", answer(&error)).contains(SECRET),
        "and neither shows the password: {error}",
    );
}

/// A command Redis holds fails at the budget, as a timeout — and the connection
/// outlives it: the reply that arrives once Redis answers again goes to nobody,
/// and the next command gets its own. The timeout says the answer did not come
/// in time, never that the command did not run, and the held write lands.
///
/// `CLIENT PAUSE WRITE` holds every client's writes, so the pause is lifted the
/// moment the timeout is measured — `CLIENT UNPAUSE` is not a write, and gets
/// through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_redis_holds_fails_at_the_budget_and_the_connection_outlives_it() {
    let budget = Duration::from_millis(300);
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: crate::redis_url(),
        connect_timeout: budget,
        ..RedisConfig::default()
    })
    .await
    .expect("connect to the dev container Redis");
    let mut admin = crate::connect().await;
    let key = crate::unique_key("held-write");

    redis::cmd("CLIENT")
        .arg("PAUSE")
        .arg(3_000)
        .arg("WRITE")
        .query_async::<()>(&mut admin)
        .await
        .expect("CLIENT PAUSE WRITE");
    let started = Instant::now();
    let held = redis::cmd("SET")
        .arg(&key)
        .arg("landed")
        .query_async::<()>(&mut conn)
        .await;
    let took = started.elapsed();
    redis::cmd("CLIENT")
        .arg("UNPAUSE")
        .query_async::<()>(&mut admin)
        .await
        .expect("CLIENT UNPAUSE");

    let error = held.expect_err("a write Redis holds is ended by the budget");
    assert!(error.is_timeout(), "it fails as a timeout: {error}");
    assert!(
        took >= budget && took < budget * 3,
        "and at the budget, took {took:?}",
    );
    assert!(
        error
            .to_string()
            .contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
        "naming the knob that sets it: {error}",
    );

    let landed: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .expect("the connection answers the next command");
    assert_eq!(
        landed.as_deref(),
        Some("landed"),
        "the held write ran once Redis answered again",
    );
    redis::cmd("DEL")
        .arg(&key)
        .query_async::<()>(&mut conn)
        .await
        .expect("DEL the probe");
}

/// A command waiting on a connection the client is reopening still ends within
/// the connect budget, as a timeout — which is what a reply timeout inside the
/// client never covers, and why the bound exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_waiting_on_a_reconnection_that_never_answers_times_out_within_the_budget() {
    let proxy = crate::DarkeningProxy::start().await;
    let budget = Duration::from_millis(500);
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: proxy.url(),
        connect_timeout: budget,
        ..RedisConfig::default()
    })
    .await
    .expect("connect through the proxy");
    redis::cmd("PING")
        .query_async::<()>(&mut conn)
        .await
        .expect("the connection answers through the proxy");

    proxy.go_dark();
    // Commands may still succeed until the client sees the drop; the first one
    // to fail is where it starts reopening the connection.
    let mut saw_the_drop = false;
    for _ in 0..50 {
        if redis::cmd("PING")
            .query_async::<()>(&mut conn)
            .await
            .is_err()
        {
            saw_the_drop = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_the_drop,
        "the proxy going dark must drop the connection"
    );

    let started = Instant::now();
    let error = redis::cmd("PING")
        .query_async::<()>(&mut conn)
        .await
        .expect_err("nothing answers once the proxy is dark");
    let took = started.elapsed();

    assert!(error.is_timeout(), "it fails as a timeout: {error}");
    assert!(
        took < budget * 2,
        "and within the budget, not after the client's own reconnection attempts: took {took:?}",
    );
    assert!(
        proxy.dials_while_dark() >= 1,
        "the command waited on a reopened connection, not on the dropped one",
    );
}

/// How long the connection has to answer again once Redis dropped it.
const RECOVERY: Duration = Duration::from_secs(5);

const LIMIT: Throttle = Throttle::new(1_000_000, Duration::from_secs(60));

/// Which holder of the connection, if any, could not reach Redis.
async fn every_holder_answers(
    conn: &RedisConnection,
    throttler: &RedisThrottler,
    key: &str,
) -> Result<(), &'static str> {
    // The rate limiter fails closed, so a denial under a limit nothing reaches
    // is Redis unreachable.
    if !throttler.hit(key, LIMIT).await.allowed {
        return Err("the rate limiter");
    }
    redis::cmd("PING")
        .query_async::<()>(&mut conn.clone())
        .await
        .map_err(|_| "a caller's own command")
}

/// Redis dropping the app's connection is survived without a reboot: the
/// client reopens it behind its holders, and the rate limiter and a caller's
/// own command answer again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_connection_answers_again_once_redis_drops_it() {
    let url = crate::redis_url_on(crate::DB_CONNECTION_DROP);
    let conn = RedisConnection::connect(&RedisConfig {
        url: url.clone(),
        ..RedisConfig::default()
    })
    .await
    .expect("connect to the isolated database");
    redis::cmd("FLUSHDB")
        .query_async::<()>(&mut conn.clone())
        .await
        .expect("start from an empty database");
    let throttler = RedisThrottler::new(conn.clone());
    let key = crate::unique_key("reconnect");

    every_holder_answers(&conn, &throttler, &key)
        .await
        .expect("every holder answers before Redis drops anything");
    // A connection nothing reopens, on the same database: it is how this test
    // knows the drop took, so a recovery below is a reconnection and never a
    // drop that missed.
    let mut unmanaged = redis::Client::open(url.as_str())
        .expect("the isolated url parses")
        .get_multiplexed_async_connection()
        .await
        .expect("open a connection nothing reopens");
    redis::cmd("PING")
        .query_async::<()>(&mut unmanaged)
        .await
        .expect("the unmanaged connection answers before the drop");

    let dropped = crate::drop_every_connection_on(crate::DB_CONNECTION_DROP).await;
    assert!(
        dropped >= 2,
        "the app's connection and the unmanaged one must be dropped, dropped {dropped}",
    );
    assert!(
        redis::cmd("PING")
            .query_async::<()>(&mut unmanaged)
            .await
            .is_err(),
        "a connection nothing reopens stays dropped, or the drop did not take",
    );

    let started = Instant::now();
    let mut outcome = Err("nothing was tried");
    while started.elapsed() < RECOVERY {
        outcome = every_holder_answers(&conn, &throttler, &key).await;
        if outcome.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if let Err(holder) = outcome {
        panic!("{holder} still could not reach Redis {RECOVERY:?} after it dropped the connection");
    }
    let recovered_after = started.elapsed();
    assert!(
        recovered_after < RECOVERY,
        "every holder answered again, but only {recovered_after:?} after the drop",
    );

    redis::cmd("FLUSHDB")
        .query_async::<()>(&mut conn.clone())
        .await
        .expect("leave the isolated database empty");
}
