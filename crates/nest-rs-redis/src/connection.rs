//! [`RedisConnection`] — the one Redis connection pool every binding in this
//! crate shares. Opened once by [`RedisModule`](crate::RedisModule) in the
//! collect phase; the queue producer, the worker and the rate-limit store each
//! read it from the container rather than opening a pool of their own.
//!
//! It sits at the crate root because three binding folders reach it: filed
//! under whichever asked first, it was named, configured and module-gated for
//! the queue, so enabling the throttler obliged an app with no queue to import
//! the queue's module and set the queue's URL.

use std::time::{Duration, Instant};

use deadpool_redis::{
    Config, Connection, CreatePoolError, Hook, Pool, PoolConfig, PoolError, Runtime, TimeoutType,
};

use crate::error::RedisError;

/// The sentence every binding's boot error appends when the connection is
/// missing — one wording, three sites, so the remedy cannot drift.
pub(crate) const CONNECTION_REMEDY: &str = "RedisConnection is not registered — import \
     RedisModule::for_root(None), which opens the one Redis connection pool every Redis binding shares";

/// Connections the pool holds at most. oxana's own default: the job runtime's
/// dispatchers, heartbeat and scans draw from this pool beside the producer and
/// the rate limiter, and deadpool's default — twice the CPUs — is two
/// connections on a one-CPU pod, which a busy throttler alone exhausts.
const POOL_SIZE: usize = 50;

/// The app's shared Redis connection pool. A `Clone` shares the pool, so every
/// binding draws from the same connections — never a second pool.
#[derive(Clone)]
pub struct RedisConnection {
    pool: Pool,
    /// The connect budget, which also bounds a caller's whole wait for a
    /// pooled connection.
    budget: Duration,
}

/// Backoff before the first retry; doubles up to [`MAX_RETRY_BACKOFF`] and is
/// always clamped to what is left of the budget.
const FIRST_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Ceiling for the doubling backoff — a whole boot budget must still fit
/// several attempts, each of which gets its own `warn`.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);

impl RedisConnection {
    /// Open the pool to `redis_url`, bounded by
    /// [`RedisConfig::connect_timeout`](crate::RedisConfig::connect_timeout)'s
    /// default.
    ///
    /// Prefer [`connect_within`](Self::connect_within) from the module factory,
    /// which passes the configured budget.
    pub async fn connect(redis_url: &str) -> Result<Self, RedisError> {
        Self::connect_within(redis_url, crate::RedisConfig::default().connect_timeout).await
    }

    /// Open the pool and prove a connection from it answers, giving up after
    /// `budget`.
    ///
    /// A pool opens nothing until it is asked, so without the proof a wrong
    /// `NESTRS_REDIS__URL` boots cleanly and fails on the first job or the first
    /// rate-limited request — and an endpoint that never answers parks the
    /// process with an empty log, never healthy and never crashed. Every attempt
    /// is announced on `nest_rs::redis`, and the budget converts the hang into
    /// the boot error `/queue/wiring/` promises.
    ///
    /// The same budget bounds what a caller waits on afterwards — a connection
    /// from the pool, and every command's reply — so a Redis that stops
    /// answering fails its caller within it rather than holding the caller.
    /// Opening a new connection is bounded tighter still, by the client's own
    /// one-second connect timeout, which no budget widens.
    ///
    /// A URL the client cannot parse, or an answer Redis will give on every
    /// attempt — refused credentials, an ACL denying the proof, a database
    /// index out of range — fails at once instead of spending the budget on
    /// retries that cannot succeed.
    pub async fn connect_within(redis_url: &str, budget: Duration) -> Result<Self, RedisError> {
        let endpoint = address(redis_url);
        let pool = open_pool(redis_url, budget).map_err(|source| RedisError::InvalidUrl {
            endpoint: endpoint.clone(),
            source,
        })?;
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
            match tokio::time::timeout(remaining, ping(&pool)).await {
                Ok(Ok(())) => {
                    if attempts > 1 {
                        tracing::info!(
                            target: crate::TARGET,
                            endpoint = %endpoint,
                            attempts,
                            "connected to redis after retrying",
                        );
                    }
                    return Ok(Self { pool, budget });
                }
                // The pool's own wait is bounded by the same budget, so its
                // timeout and ours are one event whichever fires first: a hung
                // DNS or a black-holed port is as legible as a refused connection.
                Ok(Err(PoolError::Timeout(_))) | Err(_) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        endpoint = %endpoint,
                        attempt = attempts,
                        timeout_secs = budget.as_secs(),
                        "redis connect timed out",
                    );
                    break;
                }
                Ok(Err(PoolError::Backend(source))) if refused(&source) => {
                    return Err(RedisError::Refused { endpoint, source });
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

    /// One connection from the shared pool, for a command of the caller's own.
    /// It returns to the pool on drop.
    ///
    /// The queue and the rate limiter draw from the same pool, so a **blocking
    /// command** (`BLPOP`, `WAIT`, a `SUBSCRIBE` that parks the socket) holds its
    /// connection away from them for as long as it blocks — and fails once it
    /// blocks past the connect budget, which bounds every reply. Non-blocking,
    /// atomic operations (a `Script`, `INCR`, `GET`/`SET`) are the intended
    /// traffic.
    ///
    /// The whole wait is bounded by the connect budget, not only its parts: the
    /// pool re-checks each idle connection before handing one out, and against
    /// a stalled Redis every check spends a budget of its own — a rate-limited
    /// request held for one budget per idle connection before it was denied.
    pub async fn connection(&self) -> Result<Connection, PoolError> {
        tokio::time::timeout(self.budget, self.pool.get())
            .await
            .unwrap_or(Err(PoolError::Timeout(TimeoutType::Wait)))
    }

    /// The pool itself, for the queue storage oxana runs on — a clone shares it.
    pub(crate) fn pool(&self) -> Pool {
        self.pool.clone()
    }
}

/// Whether Redis answered in a way every attempt will repeat: what redis itself
/// marks as not worth retrying — a refused `SELECT`, an ACL `NOPERM`, a socket
/// the process may not open — and refused credentials, which it retries only
/// on a new connection. A transport failure, or a server still loading its
/// dataset, is worth the budget and stays in it.
fn refused(error: &redis::RedisError) -> bool {
    error.kind() == redis::ErrorKind::AuthenticationFailed
        || matches!(error.retry_method(), redis::RetryMethod::NoRetry)
}

/// The pool, with every wait bounded by `budget`. Building it parses the URL
/// and opens nothing.
///
/// A reply is bounded by the budget too, replacing redis 1.x's own 500ms: the
/// job runtime is written for replies far slower than that, and a rate limiter
/// denying every request because Redis answered in 600ms is a second, hidden
/// threshold beside the one the operator configured.
fn open_pool(redis_url: &str, budget: Duration) -> Result<Pool, CreatePoolError> {
    let mut config = Config::from_url(redis_url);
    let mut pool = PoolConfig::new(POOL_SIZE);
    pool.timeouts.wait = Some(budget);
    pool.timeouts.create = Some(budget);
    pool.timeouts.recycle = Some(budget);
    config.pool = Some(pool);
    config
        .builder()
        .map_err(CreatePoolError::Config)?
        .post_create(Hook::sync_fn(move |conn, _| {
            conn.set_response_timeout(budget);
            Ok(())
        }))
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(CreatePoolError::Build)
}

/// One round trip over a pooled connection — the proof a pool is not.
async fn ping(pool: &Pool) -> Result<(), PoolError> {
    let mut conn = pool.get().await?;
    redis::cmd("PING")
        .query_async::<()>(&mut conn)
        .await
        .map_err(PoolError::Backend)
}

/// The endpoint the connect diagnostics name, in logs and in the boot error:
/// the address the client parsed from the URL — `host:port`, or a socket path —
/// and never the URL, which routinely embeds a password in its userinfo or its
/// query. The client's own parser decides, so what is shown is what is dialled,
/// and a URL it cannot parse is named by its scheme alone.
fn address(url: &str) -> String {
    use redis::IntoConnectionInfo;
    match url.into_connection_info() {
        Ok(info) => info.addr().to_string(),
        Err(_) => match url.split_once(':') {
            Some((scheme, _)) => format!("{scheme}://<unparseable>"),
            None => "<unparseable>".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The endpoint is what the client dials. Every shape that once reached a
    /// boot error or a `warn` with its password — a password holding `/` or
    /// `?`, a socket's `?pass=`, a URL with no `//` — shows none, whether the
    /// client parses it or not.
    #[test]
    fn the_endpoint_is_the_address_the_client_dials_and_never_a_credential() {
        for (url, shown) in [
            (
                "redis://alice:s3cr3t@redis.internal:6379/2",
                "redis.internal:6379",
            ),
            ("redis://127.0.0.1:6379/0", "127.0.0.1:6379"),
            (
                "redis+unix:///tmp/redis.sock?pass=s3cr3t",
                "/tmp/redis.sock",
            ),
            ("redis://user:s3cr3t@[::bad-host/", "redis://<unparseable>"),
            ("not-a-url", "<unparseable>"),
        ] {
            assert_eq!(address(url), shown, "{url}");
        }
        for url in [
            "redis://alice:p@s3cr3t@redis:6379",
            "redis://alice:pa/s3cr3t@127.0.0.1:9/",
            "redis://alice:pa?s3cr3t@127.0.0.1:9/",
            "unix:/tmp/redis.sock?pass=s3cr3t",
            "redis:/alice:s3cr3t@127.0.0.1:9/0",
            "redis:\\\\alice:s3cr3t@127.0.0.1:9/0",
        ] {
            let shown = address(url);
            assert!(!shown.contains("s3cr3t"), "{url} showed {shown}");
        }
    }

    /// Every retry costs the boot a backoff and the operator a line telling
    /// them to widen the budget, so an answer that repeats must not get one —
    /// and a transport failure, which may clear, must.
    #[test]
    fn an_answer_redis_repeats_is_refused_and_a_transport_failure_is_retried() {
        let credentials = redis::RedisError::from((
            redis::ErrorKind::AuthenticationFailed,
            "Password authentication failed",
        ));
        let config = redis::RedisError::from((
            redis::ErrorKind::InvalidClientConfig,
            "invalid database index",
        ));
        let transport =
            redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        assert!(refused(&credentials), "refused credentials fail at once");
        assert!(
            refused(&config),
            "a configuration redis will not retry fails at once"
        );
        assert!(!refused(&transport), "a refused TCP connection may clear");
    }

    /// C6: an unreachable backend used to park the process forever with zero
    /// output. The budget must convert that into a bounded, named boot error —
    /// and every attempt inside it must say so, with the credentials redacted
    /// before they reach the log.
    #[tokio::test]
    async fn a_refused_endpoint_announces_every_attempt_and_fails_naming_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let started = Instant::now();
        // Port 9 is `discard` — reserved and never listening, so every attempt
        // is refused at once. The budget sits above `FIRST_RETRY_BACKOFF` (250ms)
        // so the loop retries at least once before it expires.
        let Err(err) = RedisConnection::connect_within(
            "redis://alice:s3cr3t@127.0.0.1:9/0",
            Duration::from_millis(700),
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

        let retries = logs.find(
            crate::TARGET,
            "redis unreachable — retrying within the connect budget",
        );
        assert!(
            retries.len() > 1,
            "every attempt is announced: {:#?}",
            logs.events(),
        );
        for event in &retries {
            assert_eq!(event.level, "warn");
            let endpoint = event
                .field("endpoint")
                .expect("the event names the endpoint");
            assert!(
                endpoint.contains("127.0.0.1:9"),
                "the addressable part is what the operator needs: {endpoint}",
            );
            assert!(
                !endpoint.contains("s3cr3t"),
                "and the credentials are redacted before they reach the log: {endpoint}",
            );
            assert!(event.field("attempt").is_some(), "{:?}", event.fields);
        }
    }

    /// A URL the client cannot parse fails the same way on every attempt, so
    /// the boot refuses it at once — naming the endpoint, without the password —
    /// instead of retrying for the whole budget.
    #[tokio::test]
    async fn a_rejected_url_fails_at_once_with_the_credentials_redacted() {
        let started = Instant::now();
        let Err(err) = RedisConnection::connect_within(
            "redis://user:secret@[::bad-host/",
            Duration::from_secs(5),
        )
        .await
        else {
            panic!("a URL the client rejects must fail the boot")
        };

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a deterministic failure spends none of the budget, took {:?}",
            started.elapsed(),
        );
        let rendered = err.to_string();
        assert!(
            rendered.contains("redis://<unparseable>"),
            "the error names the URL by its scheme: {rendered}",
        );
        assert!(
            !rendered.contains("secret"),
            "the error must not leak the password: {rendered}",
        );
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis", "URL")),
            "the error names the variable to fix: {rendered}",
        );
    }

    /// A listener that accepts and never answers: the connection opens, and the
    /// round trip that would prove it never returns.
    async fn silent_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a silent listener");
        let addr = listener.local_addr().expect("the listener's address");
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        (addr, silent)
    }

    /// The other branch: the budget elapses mid-attempt, so a hung DNS or a
    /// black-holed port is as legible as a refused connection. Without it the
    /// process reports nothing at all for the whole budget and then fails.
    #[tokio::test]
    async fn a_budget_that_expires_mid_attempt_is_its_own_line() {
        let logs = nest_rs_testing::LogCapture::install();
        let (addr, silent) = silent_listener().await;

        assert!(
            RedisConnection::connect_within(
                &format!("redis://{addr}/"),
                Duration::from_millis(300)
            )
            .await
            .is_err(),
            "an endpoint that never answers fails the boot rather than parking it",
        );
        silent.abort();

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

    /// A budget shorter than the client's one-second connect timeout ends the
    /// opening of a connection itself: the pool's own `Timeout(Create)`, not
    /// the client's `timed out` a second later.
    #[tokio::test]
    async fn the_pool_bounds_opening_a_connection_by_the_budget() {
        let (addr, silent) = silent_listener().await;
        let pool = open_pool(&format!("redis://{addr}/"), Duration::from_millis(200))
            .expect("a well-formed URL builds a pool");

        let started = Instant::now();
        let outcome = pool.get().await;
        let took = started.elapsed();
        silent.abort();

        assert!(
            matches!(outcome, Err(PoolError::Timeout(_))),
            "the pool's own timeout ends the wait: {:?}",
            outcome.err(),
        );
        assert!(
            took < Duration::from_millis(900),
            "and it ends inside the budget, took {took:?}",
        );
    }
}
