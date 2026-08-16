//! Shared Redis connection + typed [`Queue`] producer handle. The queue name
//! supplied at the call site must match the consuming `#[process(queue = ...)]`.
//!
//! Wire format is a JSON **envelope** — `{ "v": <number>, "payload": <user
//! payload> }` — pushed onto an apalis `RedisStorage<serde_json::Value>`. The
//! matching consumer (the `#[processor]` macro-emitted `JobHandler`) unwraps
//! the envelope, switches on `v`, and deserializes `payload` to the user's
//! job type. Unversioned legacy values are decoded directly with a warning so
//! a rolling deploy doesn't drop jobs left in Redis from the prior release.
//! This is the seam that lets the `#[processor]` macro stay backend-agnostic:
//! any backend can drain the `ProcessMethod` inventory because every job is a
//! JSON `Value` at the boundary.

use std::marker::PhantomData;
use std::time::{Duration, Instant};

use apalis::prelude::Storage;
use apalis_redis::{Config, RedisStorage};
use async_trait::async_trait;
use nest_rs_queue::{Job, JobProducer, QueueError, WIRE_FORMAT_VERSION};
use redis::aio::ConnectionManager;
use serde_json::json;

use crate::error::RedisError;

/// The app's shared Redis connection — queue-flavoured by history, not
/// queue-only. It is seeded once by [`QueueModule`](crate::QueueModule) and
/// injected by producers; other Redis-backed features enabled on this crate
/// (the `throttler` rate-limit store, a future cache/locks) reuse the very same
/// multiplexed handle via [`manager`](Self::manager) instead of opening a
/// second connection.
#[derive(Clone)]
pub struct QueueConnection {
    conn: ConnectionManager,
}

/// Backoff before the first retry; doubles up to [`MAX_RETRY_BACKOFF`] and is
/// always clamped to what is left of the budget.
const FIRST_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Ceiling for the doubling backoff — a whole boot budget must still fit
/// several attempts, each of which gets its own `warn`.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);

impl QueueConnection {
    /// Open a multiplexed Redis connection to `redis_url`, bounded by
    /// [`QueueConfig::connect_timeout`](crate::QueueConfig::connect_timeout)'s
    /// default.
    ///
    /// Prefer [`connect_within`](Self::connect_within) from the module factory,
    /// which passes the configured budget.
    pub async fn connect(redis_url: &str) -> Result<Self, RedisError> {
        Self::connect_within(redis_url, crate::QueueConfig::default().connect_timeout).await
    }

    /// Open the connection, giving up after `budget`.
    ///
    /// The underlying client retries an unreachable endpoint indefinitely and
    /// silently, which turned a misconfigured `NESTRS_QUEUE__URL` into a
    /// process parked forever with an empty log — the worst shape for a
    /// container platform, since it never becomes healthy and never crashes.
    /// Every attempt is announced on `nest_rs::queue` and the budget converts
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
                            target: "nest_rs::queue",
                            endpoint = %endpoint,
                            attempts,
                            "connected to the queue backend after retrying",
                        );
                    }
                    return Ok(Self { conn });
                }
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: "nest_rs::queue",
                        endpoint = %endpoint,
                        attempt = attempts,
                        error = %error,
                        "queue backend unreachable — retrying within the connect budget",
                    );
                    last_error = Some(error);
                }
                // The budget elapsed mid-attempt: one final warn so a hung DNS
                // or a black-holed port is as legible as a refused connection.
                Err(_elapsed) => {
                    tracing::warn!(
                        target: "nest_rs::queue",
                        endpoint = %endpoint,
                        attempt = attempts,
                        timeout_secs = budget.as_secs(),
                        "queue backend connect timed out",
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

    /// A cheap clone of the multiplexed connection handle, shared with
    /// non-queue Redis features enabled on this crate (rate-limit store, cache,
    /// distributed locks). `ConnectionManager` is `Clone` and every clone talks
    /// over the one underlying connection, so this is the reuse seam — no second
    /// connect.
    ///
    /// **Do not run blocking commands on it** (`BLPOP`, `BRPOP`, `WAIT`, a
    /// `SUBSCRIBE` that parks the socket): the handle multiplexes every caller
    /// over a single connection, so a blocking command would stall all other
    /// users. Non-blocking, atomic operations (a `Script`, `INCR`, `GET`/`SET`)
    /// are the intended traffic.
    pub fn manager(&self) -> ConnectionManager {
        self.conn.clone()
    }

    /// Typed producer handle. `J` is the job type the consumer expects; the
    /// payload is serialized to JSON on the wire (matches the consumer's
    /// `JobHandler` deserializing from `serde_json::Value`).
    pub fn of<J: Job>(&self, queue: &str) -> Queue<J> {
        Queue {
            storage: self.value_storage(queue),
            _phantom: PhantomData,
        }
    }

    /// Producer-side storage handle. Configured to namespace under `queue`
    /// just like the consumer — this is how apalis routes a job to the right
    /// worker.
    pub(crate) fn value_storage(&self, queue: &str) -> RedisStorage<serde_json::Value> {
        RedisStorage::new_with_config(self.conn.clone(), Config::default().set_namespace(queue))
    }

    /// Consumer-side storage, one job per fetch. A `#[process]` method runs a
    /// single job at a time (see [`QueueWorker`](crate::QueueWorker)), so
    /// prefetching would only hold jobs a peer replica could be running.
    pub(crate) fn consumer_storage(&self, queue: &str) -> RedisStorage<serde_json::Value> {
        RedisStorage::new_with_config(
            self.conn.clone(),
            Config::default().set_namespace(queue).set_buffer_size(1),
        )
    }
}

/// Typed producer handle returned by [`QueueConnection::of`]. The `J` is a
/// compile-time aid for the call site — the wire payload is always JSON.
pub struct Queue<J: Job> {
    storage: RedisStorage<serde_json::Value>,
    _phantom: PhantomData<fn(J)>,
}

impl<J: Job> Queue<J> {
    /// Serialize `job` and enqueue it onto this queue's Redis storage.
    pub async fn push(&self, job: J) -> Result<(), QueueError> {
        let payload = serde_json::to_value(&job)?;
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so clone per call rather than force callers to hold it mut.
        let mut storage = self.storage.clone();
        storage
            .push(envelope(payload))
            .await
            .map_err(QueueError::backend)?;
        Ok(())
    }
}

/// Backend-agnostic producer surface — any feature injecting
/// `Arc<dyn JobProducer>` (instead of the concrete `QueueConnection`) is
/// portable across backends.
#[async_trait]
impl JobProducer for QueueConnection {
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> Result<(), QueueError> {
        let mut storage = self.value_storage(queue);
        storage
            .push(envelope(payload))
            .await
            .map_err(QueueError::backend)?;
        Ok(())
    }
}

/// Wrap a user payload in the wire envelope the consumer expects. Bumping
/// [`WIRE_FORMAT_VERSION`] lets a rolling deploy fail closed instead of
/// misinterpreting bytes.
fn envelope(payload: serde_json::Value) -> serde_json::Value {
    json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": payload,
    })
}

/// Replace any `user:password@` userinfo with `***`. The connect diagnostics
/// name the endpoint in logs and in the boot error, and `NESTRS_QUEUE__URL`
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
        let Err(err) = QueueConnection::connect_within(
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
            rendered.contains("NESTRS_QUEUE__CONNECT_TIMEOUT_SECS"),
            "the error names the knob that widens the budget: {rendered}",
        );
    }

    /// A wrong `NESTRS_QUEUE__URL` used to park the process forever with an
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
            QueueConnection::connect_within(
                "redis://user:secret@[::bad-host/",
                Duration::from_millis(700),
            )
            .await
            .is_err(),
            "a URL the client rejects fails the boot rather than parking it",
        );

        let retries = logs.find(
            "nest_rs::queue",
            "queue backend unreachable — retrying within the connect budget",
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
            QueueConnection::connect_within("redis://127.0.0.1:1/", Duration::from_millis(120))
                .await
                .is_err(),
            "an unreachable endpoint fails the boot rather than parking it",
        );

        let expired = logs
            .find("nest_rs::queue", "queue backend connect timed out")
            .into_iter()
            .next()
            .expect("the budget expiring is its own line, not a silent give-up");
        assert_eq!(expired.level, "warn");
        assert!(
            expired.field("timeout_secs").is_some(),
            "{:?}",
            expired.fields
        );
        assert!(
            expired
                .field("endpoint")
                .is_some_and(|e| e.contains("127.0.0.1:1")),
            "{:?}",
            expired.fields,
        );
    }
}
