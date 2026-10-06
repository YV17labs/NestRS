//! The shared connection against a live Redis — the branches a local listener
//! cannot reach, because they need a server that answers: one that refuses
//! (credentials, a database index, an ACL), one that answers with what may
//! clear (busy, loading, failing over) and then serves, one with a single
//! client slot left, one that holds a command, one that vanishes behind a
//! network still accepting the dial, and one that drops every connection the
//! app holds.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError, RedisThrottler};
use nest_rs_throttler::{Throttle, ThrottlerStore};

use crate::harness::AT_ONCE;
use crate::harness::connection::{
    NOT_READY, ScriptedRedis, answer, config, database_refused_at_once,
};

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

/// Credentials Redis refuses are not an outage. Retried as one, the boot spent
/// its whole budget and then told the operator to check the network and widen
/// the timeout.
#[tokio::test]
async fn credentials_redis_refuses_fail_the_boot_at_once_naming_them() {
    let url = crate::url_as(&crate::redis_url(), "", "nestrs-e2e-wrong-secret");
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

    let error = database_refused_at_once(
        crate::redis_url_on(count),
        i64::from(count),
        "a database index out of range",
    )
    .await;
    assert!(
        answer(&error).contains("switch database"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
}

/// A test moves onto a database of its own through the suite's URL, which the
/// deployment writes: its query and fragment — `?protocol=resp3` — stay with
/// it, and the index replaces the URL's path wherever the authority ends.
/// config-1r2: an ACL user without `+select` on a URL naming a database — the
/// common least-privilege shape — was retried for the whole budget, because the
/// client drops the `NOPERM` from a refused `SELECT`, and then reported as a
/// Redis "not ready" told to widen the budget. It is refused at once, naming
/// the index.
#[tokio::test]
async fn an_acl_denying_select_fails_the_boot_at_once_naming_the_index() {
    const SECRET: &str = "nestrs-e2e-acl-select-secret";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let user = format!("nestrs-e2e-no-select-{}-{nanos}", std::process::id());
    let mut admin = crate::connect().await;
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&user)
        .arg("on")
        .arg(format!(">{SECRET}"))
        .arg("+@all")
        .arg("-select")
        .arg("~*")
        .query_async::<()>(&mut admin)
        .await
        .expect("ACL SETUSER");

    // Refused at the `SELECT`, so the index reaches no key: any but 0 sends one.
    let url = crate::url_as(
        &crate::redis_url_on(crate::DB_CONFINED_TO_THE_PREFIX),
        &user,
        SECRET,
    );
    let outcome = tokio::time::timeout(
        AT_ONCE * 2,
        database_refused_at_once(
            url,
            i64::from(crate::DB_CONFINED_TO_THE_PREFIX),
            "an ACL user without +select",
        ),
    )
    .await;
    redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&user)
        .query_async::<i64>(&mut admin)
        .await
        .expect("ACL DELUSER");
    let error = outcome.expect("refused at once, not retried for the budget");
    assert!(
        answer(&error).contains("no permissions"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
    assert!(
        !format!("{error} {}", answer(&error)).contains(SECRET),
        "and neither shows the password: {error}",
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

    let url = crate::url_as(&crate::redis_url(), &user, SECRET);
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
        answer(&error).contains("no permissions to run the 'ping' command"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
    assert!(
        !format!("{error} {}", answer(&error)).contains(SECRET),
        "and neither shows the password: {error}",
    );
}

/// A Redis that answers but is not ready — busy running a script past its
/// threshold, loading its dataset, failing over, or answering a code the client
/// does not know — is retried until it is, within the budget. `BUSY` and an
/// unknown code used to fail the boot in milliseconds as a refusal, telling the
/// operator to check the URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_not_ready_yet_is_retried_until_it_serves() {
    for line in NOT_READY {
        let proxy = ScriptedRedis::start(Some(crate::redis_address()), None).await;
        proxy.answer_with(Some(line));
        let ready = tokio::spawn({
            let answer = Arc::clone(&proxy.answer);
            async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                *answer.lock().expect("answer lock") = None;
            }
        });
        let started = Instant::now();
        let outcome = RedisConnection::connect(&RedisConfig {
            url: proxy.url(),
            connect_timeout: Duration::from_secs(10),
            ..RedisConfig::default()
        })
        .await;
        let took = started.elapsed();
        ready.await.expect("the proxy is made ready");
        let mut conn = outcome.unwrap_or_else(|error| panic!("{line}: {error:#}"));
        assert!(
            took >= Duration::from_millis(500),
            "{line}: the boot waited for Redis rather than connecting past it, took {took:?}"
        );
        redis::cmd("PING")
            .query_async::<()>(&mut conn)
            .await
            .expect("the kept connection serves");
    }
}

/// The one answer to a `SELECT` that clears — a server busy running a script —
/// is retried until it serves, as it is at the proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_select_met_by_a_busy_server_is_retried_until_it_serves() {
    let proxy = ScriptedRedis::start(Some(crate::redis_address()), None).await;
    proxy.answer_with(Some(NOT_READY[0]));
    let ready = tokio::spawn({
        let answer = Arc::clone(&proxy.answer);
        async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            *answer.lock().expect("answer lock") = None;
        }
    });
    let started = Instant::now();
    let outcome = RedisConnection::connect(&RedisConfig {
        // Any index but 0 makes the client send a `SELECT`; this one's test
        // keeps keys under a prefix, and this connection writes none.
        url: format!("{}{}", proxy.url(), crate::DB_CONFINED_TO_THE_PREFIX),
        connect_timeout: Duration::from_secs(10),
        ..RedisConfig::default()
    })
    .await;
    let took = started.elapsed();
    ready.await.expect("the proxy is made ready");
    let mut conn = outcome.unwrap_or_else(|error| panic!("{error:#}"));
    assert!(
        took >= Duration::from_millis(500),
        "the boot waited for Redis rather than connecting past it, took {took:?}"
    );
    redis::cmd("PING")
        .query_async::<()>(&mut conn)
        .await
        .expect("the kept connection serves");
}

/// The budget's ceiling is a budget the kernel accepts: the socket's liveness
/// is set from it, and past the kernel's keepalive limit every dial failed with
/// `EINVAL` against a Redis that answered. Both waits are bounded below the
/// ceiling, so neither an absent Redis nor that regression holds the run for an
/// hour.
#[tokio::test]
async fn the_ceiling_budget_connects_and_serves() {
    crate::connect().await;
    let dialled = tokio::time::timeout(
        Duration::from_secs(30),
        RedisConnection::connect(&RedisConfig {
            url: crate::redis_url(),
            connect_timeout: Duration::from_secs(60 * 60),
            ..RedisConfig::default()
        }),
    )
    .await
    .expect("a Redis that answered the default budget answers the ceiling's at once");
    let mut conn = dialled.expect("a budget of an hour dials");
    redis::cmd("PING")
        .query_async::<()>(&mut conn)
        .await
        .expect("and serves");
}

/// A Redis with one client slot left boots: the proof is closed before the
/// connection the app keeps is opened. Held across it, every attempt needed two
/// slots, and the boot spent its whole budget before blaming the network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_with_one_client_slot_left_boots() {
    let proxy = ScriptedRedis::start(Some(crate::redis_address()), Some(1)).await;
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: proxy.url(),
        connect_timeout: Duration::from_secs(5),
        ..RedisConfig::default()
    })
    .await
    .unwrap_or_else(|error| panic!("one free slot is enough to boot: {error:#}"));
    redis::cmd("PING")
        .query_async::<()>(&mut conn)
        .await
        .expect("the kept connection serves");
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
#[expect(
    clippy::map_err_ignore,
    reason = "the test names which holder failed; the cause is the next assertion's"
)]
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
