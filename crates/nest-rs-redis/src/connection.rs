//! [`RedisConnection`] — the one multiplexed Redis handle every binding in this
//! crate shares. Opened once by [`RedisModule`](crate::RedisModule) in the
//! collect phase; the queue producer, the worker and the rate-limit store each
//! read it from the container rather than opening a second socket.
//!
//! It sits at the crate root because three binding folders reach it: filed
//! under whichever asked first, it was named, configured and module-gated for
//! the queue, so enabling the throttler obliged an app with no queue to import
//! the queue's module and set the queue's URL.

use std::time::{Duration, Instant};

use redis::aio::ConnectionManager;

use crate::error::RedisError;

/// The sentence every binding's boot error appends when the connection is
/// missing — one wording, three sites, so the remedy cannot drift.
pub(crate) const CONNECTION_REMEDY: &str = "RedisConnection is not registered — import \
     RedisModule::for_root(None), which opens the one Redis connection every Redis binding shares";

/// The app's shared Redis connection. A `Clone` is a handle clone over the one
/// underlying multiplexed socket, so every binding talks through a single
/// connection — no second connect.
#[derive(Clone)]
pub struct RedisConnection {
    conn: ConnectionManager,
}

/// Backoff before the first retry; doubles up to [`MAX_RETRY_BACKOFF`] and is
/// always clamped to what is left of the budget.
const FIRST_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Ceiling for the doubling backoff — a whole boot budget must still fit
/// several attempts, each of which gets its own `warn`.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);

impl RedisConnection {
    /// Open a multiplexed Redis connection to `redis_url`, bounded by
    /// [`RedisConfig::connect_timeout`](crate::RedisConfig::connect_timeout)'s
    /// default.
    ///
    /// Prefer [`connect_within`](Self::connect_within) from the module factory,
    /// which passes the configured budget.
    pub async fn connect(redis_url: &str) -> Result<Self, RedisError> {
        Self::connect_within(redis_url, crate::RedisConfig::default().connect_timeout).await
    }

    /// Open the connection, giving up after `budget`.
    ///
    /// The underlying client retries an unreachable endpoint indefinitely and
    /// silently, which turned a misconfigured `NESTRS_REDIS__URL` into a
    /// process parked forever with an empty log — the worst shape for a
    /// container platform, since it never becomes healthy and never crashes.
    /// Every attempt is announced on `nest_rs::redis` and the budget converts
    /// the hang into the boot error `/queue/wiring/` promises.
    pub async fn connect_within(redis_url: &str, budget: Duration) -> Result<Self, RedisError> {
        let endpoint = redact_url(redis_url);
        let deadline = Instant::now() + budget;
        let mut backoff = FIRST_RETRY_BACKOFF;
        let mut attempts = 0u32;
        let mut last_error = None;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            attempts += 1;
            match tokio::time::timeout(remaining, apalis_redis::connect(redis_url)).await {
                Ok(Ok(conn)) => {
                    if attempts > 1 {
                        tracing::info!(
                            target: crate::TARGET,
                            endpoint = %endpoint,
                            attempts,
                            "connected to redis after retrying",
                        );
                    }
                    return Ok(Self { conn });
                }
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        endpoint = %endpoint,
                        attempt = attempts,
                        error = %error,
                        "redis unreachable — retrying within the connect budget",
                    );
                    last_error = Some(error);
                }
                // The budget elapsed mid-attempt: one final warn so a hung DNS
                // or a black-holed port is as legible as a refused connection.
                Err(_elapsed) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        endpoint = %endpoint,
                        attempt = attempts,
                        timeout_secs = budget.as_secs(),
                        "redis connect timed out",
                    );
                    break;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(backoff.min(left)).await;
            backoff = (backoff * 2).min(MAX_RETRY_BACKOFF);
        }

        Err(RedisError::Unreachable {
            endpoint,
            budget,
            attempts,
            source: last_error,
        })
    }

    /// A cheap clone of the multiplexed connection handle — the reuse seam for
    /// every binding (queue storage, the rate-limit script, a future cache or
    /// lock). `ConnectionManager` is `Clone` and every clone talks over the one
    /// underlying connection, so this is never a second connect.
    ///
    /// **Do not run blocking commands on it** (`BLPOP`, `BRPOP`, `WAIT`, a
    /// `SUBSCRIBE` that parks the socket): the handle multiplexes every caller
    /// over a single connection, so a blocking command would stall all other
    /// users. Non-blocking, atomic operations (a `Script`, `INCR`, `GET`/`SET`)
    /// are the intended traffic.
    pub fn manager(&self) -> ConnectionManager {
        self.conn.clone()
    }
}

/// Replace any `user:password@` userinfo with `***`. The connect diagnostics
/// name the endpoint in logs and in the boot error, and `NESTRS_REDIS__URL`
/// routinely embeds a password.
fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, tail) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    match authority.rsplit_once('@') {
        Some((_userinfo, host)) => format!("{scheme}://***@{host}{tail}"),
        None => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_strips_userinfo_and_keeps_the_addressable_part() {
        assert_eq!(
            redact_url("redis://alice:s3cr3t@redis.internal:6379/2"),
            "redis://***@redis.internal:6379/2",
        );
        // A password containing `@` still leaves only the host visible.
        assert_eq!(
            redact_url("redis://alice:p@ss@redis:6379"),
            "redis://***@redis:6379",
        );
    }

    #[test]
    fn redact_url_leaves_a_credential_free_url_untouched() {
        assert_eq!(
            redact_url("redis://127.0.0.1:6379/0"),
            "redis://127.0.0.1:6379/0"
        );
        assert_eq!(redact_url("not-a-url"), "not-a-url");
    }

    // C6: an unreachable backend used to park the process forever with zero
    // output. The budget must convert that into a bounded, named boot error.
    #[tokio::test]
    async fn connect_within_gives_up_on_an_unreachable_endpoint_and_names_it() {
        let started = Instant::now();
        // Port 9 is `discard` — reserved and never listening.
        let Err(err) = RedisConnection::connect_within(
            "redis://alice:s3cr3t@127.0.0.1:9/0",
            Duration::from_millis(600),
        )
        .await
        else {
            panic!("an unreachable endpoint must not connect, and must not hang")
        };

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the connect budget must bound the wait, took {:?}",
            started.elapsed(),
        );
        let rendered = err.to_string();
        assert!(
            rendered.contains("127.0.0.1:9"),
            "the error names the endpoint: {rendered}",
        );
        assert!(
            !rendered.contains("s3cr3t"),
            "the error must not leak the password: {rendered}",
        );
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
            "the error names the knob that widens the budget: {rendered}",
        );
    }

    /// A wrong `NESTRS_REDIS__URL` used to park the process forever with an
    /// empty log — never healthy, never crashed, which is the worst shape a
    /// container platform can be handed.
    ///
    /// The budget turns that into a boot error; these two events turn the boot
    /// error into a diagnosis. They need **two** tests because the loop can
    /// only take one branch per run: a URL the client rejects errors instantly
    /// and announces every attempt, while an endpoint it merely cannot reach
    /// hangs — `apalis_redis::connect` retries a refused port internally and
    /// silently, which is the exact behaviour the budget exists to bound.
    #[tokio::test]
    async fn a_rejected_url_announces_every_attempt_with_the_credentials_redacted() {
        let logs = nest_rs_testing::LogCapture::install();
        // The budget sits above `FIRST_RETRY_BACKOFF` (250ms) so the loop
        // retries at least once before it expires.
        assert!(
            RedisConnection::connect_within(
                "redis://user:secret@[::bad-host/",
                Duration::from_millis(700),
            )
            .await
            .is_err(),
            "a URL the client rejects fails the boot rather than parking it",
        );

        let retries = logs.find(
            crate::TARGET,
            "redis unreachable — retrying within the connect budget",
        );
        assert!(
            !retries.is_empty(),
            "every attempt is announced: {:#?}",
            logs.events(),
        );
        for event in &retries {
            assert_eq!(event.level, "warn");
            let endpoint = event
                .field("endpoint")
                .expect("the event names the endpoint");
            assert!(
                endpoint.contains("bad-host"),
                "the addressable part is what the operator needs: {endpoint}",
            );
            assert!(
                !endpoint.contains("secret"),
                "and the credentials are redacted before they reach the log: {endpoint}",
            );
            assert!(event.field("attempt").is_some(), "{:?}", event.fields);
        }
    }

    /// The other branch: the budget elapses mid-attempt, so a hung DNS or a
    /// black-holed port is as legible as a refused connection. Without it the
    /// process reports nothing at all for the whole budget and then fails.
    #[tokio::test]
    async fn a_budget_that_expires_mid_attempt_is_its_own_line() {
        let logs = nest_rs_testing::LogCapture::install();
        // Port 1 on loopback: the client keeps retrying inside a single
        // `connect` call, so the budget is what ends it.
        assert!(
            RedisConnection::connect_within("redis://127.0.0.1:1/", Duration::from_millis(120))
                .await
                .is_err(),
            "an unreachable endpoint fails the boot rather than parking it",
        );

        let expired = logs
            .find(crate::TARGET, "redis connect timed out")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("the expiry is its own event: {:#?}", logs.events()));
        assert_eq!(expired.level, "warn");
        assert!(
            expired.field("timeout_secs").is_some(),
            "the event names the budget that elapsed: {:?}",
            expired.fields,
        );
    }
}
