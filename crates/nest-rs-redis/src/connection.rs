//! [`RedisConnection`] — the one connection every binding in this crate shares
//! to reach Redis. Opened once by [`RedisModule`](crate::RedisModule) in the
//! collect phase; the queue producer, the worker and the rate-limit store each
//! read it from the container rather than opening a socket of their own.
//!
//! **It is a connection, not a factory of them.** It implements
//! [`ConnectionLike`], so the queue and the rate limiter run their scripts on a
//! clone, and a caller's own command runs on one too. Underneath is one `redis` [`ConnectionManager`]: one multiplexed
//! socket, reopened behind its callers when Redis drops it.
//!
//! **Every command a caller waits on answers or fails within the connect
//! budget**, end to end — the wait for a reopened connection included, which a
//! reply timeout inside the client would never cover. A failure Redis reports —
//! a dropped connection, a refusal — arrives when it happens; the budget running
//! out arrives as a timeout (`redis::RedisError::is_timeout`). Without the bound
//! an outage held every caller: the rate limiter's request, a push, a cancel.
//!
//! **A blocking command gets a connection of its own**
//! ([`RedisConnection::dedicated`]), opened from the same client: on the shared
//! socket it would stall every other caller for as long as it blocks. The
//! worker's read of a queue is one, bounded by its own wait plus the budget.
//!
//! **What the URL and [`RedisTls`](crate::RedisTls) say about TLS holds for every
//! connection the client opens**, the ones it reopens behind its callers
//! included: both are read into the one client the connection is opened from.
//!
//! It sits at the crate root because three binding folders reach it: filed
//! under whichever asked first, it was named, configured and module-gated for
//! the queue, so enabling the throttler obliged an app with no queue to import
//! the queue's module and set the queue's URL.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use nest_rs_config::Namespaced;
use redis::AsyncConnectionConfig;
use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig};
use redis::io::tcp::TcpSettings;
use redis::io::tcp::socket2::TcpKeepalive;
use redis::{
    Cmd, ConnectionAddr, ErrorKind, FromRedisValue, IntoConnectionInfo, Pipeline, RedisFuture,
    ScriptInvocation, ServerErrorKind, Value,
};

use crate::error::RedisError;
use crate::{RedisConfig, tls};

/// The sentence every binding's boot error appends when the connection is
/// missing — one wording, three sites, so the remedy cannot drift.
pub(crate) const CONNECTION_REMEDY: &str = "RedisConnection is not registered — import \
     RedisModule::for_root(None), which opens the one Redis connection every Redis binding shares";

/// How many times one call loads its script, at most: `redis` loads it once,
/// and a `SCRIPT FLUSH` or a failover landing between that load and its retry
/// would otherwise fail the call.
const SCRIPT_LOADS: u32 = 3;

/// The app's shared Redis connection, and a connection in its own right: it
/// implements [`ConnectionLike`], so a clone runs any `redis` command, script or
/// pipeline — pass `&mut` the clone to `query_async` or `invoke_async`. A clone
/// shares the one underlying socket, so every binding and every caller reaches
/// Redis through it, never a second one.
///
/// A command answers or fails within the connect budget; when the budget is
/// what ends it, the error is a timeout (`redis::RedisError::is_timeout`). A
/// timeout says the answer did not arrive in time, never that the command did
/// not run: a command Redis was holding still runs once Redis answers again,
/// and its late reply goes to nobody while the next command gets its own.
///
/// Every holder multiplexes over one socket and one session — the queue and the
/// rate limiter included. Redis answers one connection's commands in order, so a
/// **blocking command** (`BLPOP`, `WAIT`, a `SUBSCRIBE`) stalls every holder for
/// as long as it blocks, and fails at the budget besides; and a **command that
/// changes the session** (`SELECT`, `WATCH`, a `MULTI` sent outside an atomic
/// pipeline, `CLIENT SETNAME`, `AUTH`) changes it for every holder. Open a
/// `redis::Client` of your own for those. Non-blocking, atomic operations (a
/// `Script`, `INCR`, `GET`/`SET`, an atomic pipeline) are the intended traffic.
#[derive(Clone)]
pub struct RedisConnection {
    /// The connection every clone shares.
    kept: Arc<Kept>,
    /// How long a command on this handle waits for its answer.
    budget: Duration,
}

/// What every clone of a connection shares: the manager its commands go
/// through, and what opening it again needs.
///
/// **A connection answered `READONLY` is opened again.** A failover demotes the
/// primary under its clients: the socket stays open, the name the URL dials now
/// reaches the new primary, and `redis` reopens a connection only when its
/// socket fails — so every write would fail until the process restarted. The
/// first `READONLY` opens a manager afresh, once at a time, and every clone
/// takes it for its next command.
struct Kept {
    /// The manager, and how many times it was opened again: a read-only answer
    /// reopens only the manager that gave it, never its successor.
    manager: RwLock<(u64, ConnectionManager)>,
    /// The client the connection was opened from, which a reopened or
    /// [`dedicated`](RedisConnection::dedicated) one is opened from too: same
    /// address, same TLS material and verification.
    client: redis::Client,
    endpoint: String,
    /// The connect budget, which bounds opening the connection again.
    budget: Duration,
    /// TLS refusals met reopening the connection — `None` over plaintext, where
    /// no handshake can be refused.
    refusals: Option<Arc<TlsRefusals>>,
    reopening: AtomicBool,
}

impl Kept {
    fn new(
        manager: ConnectionManager,
        client: redis::Client,
        endpoint: String,
        budget: Duration,
        refusals: Option<Arc<TlsRefusals>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            manager: RwLock::new((0, manager)),
            client,
            endpoint,
            budget,
            refusals,
            reopening: AtomicBool::new(false),
        })
    }

    /// The manager commands go through now, and its opening.
    fn manager(&self) -> (u64, ConnectionManager) {
        self.manager
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Read what a command through the manager of `opening` met.
    fn observe<T>(self: &Arc<Self>, opening: u64, outcome: &Result<T, redis::RedisError>) {
        if let Some(refusals) = &self.refusals {
            refusals.observe(outcome);
        }
        if let Err(error) = outcome
            && error.code() == Some("READONLY")
        {
            self.reopen(opening);
        }
    }

    /// Open the connection afresh, unless the manager of `opening` was replaced
    /// already or an opening is under way.
    fn reopen(self: &Arc<Self>, opening: u64) {
        if self.manager().0 != opening || self.reopening.swap(true, Ordering::SeqCst) {
            return;
        }
        let kept = Arc::clone(self);
        tokio::spawn(async move {
            let opened = bounded(
                kept.budget,
                ConnectionManager::new_with_config(
                    kept.client.clone(),
                    manager_config(kept.budget),
                ),
            )
            .await;
            match opened {
                Ok(Ok(manager)) => {
                    let mut kept_manager =
                        kept.manager.write().unwrap_or_else(PoisonError::into_inner);
                    // Replaced meanwhile: the read-only answer was its
                    // predecessor's.
                    if kept_manager.0 != opening {
                        drop(kept_manager);
                        kept.reopening.store(false, Ordering::SeqCst);
                        return;
                    }
                    *kept_manager = (opening + 1, manager);
                    drop(kept_manager);
                    tracing::warn!(
                        target: crate::TARGET,
                        endpoint = %kept.endpoint,
                        "redis connection opened again: the server it reached answered as a \
                         read-only replica",
                    );
                }
                Ok(Err(error)) | Err(error) => tracing::warn!(
                    target: crate::TARGET,
                    endpoint = %kept.endpoint,
                    error = %nest_rs_core::error_message(&error),
                    "redis connection answered as a read-only replica and not opened again; \
                     the next read-only answer tries again",
                ),
            }
            kept.reopening.store(false, Ordering::SeqCst);
        });
    }
}

/// Whether the connection has been refused by TLS since it last answered, and
/// how to learn why.
///
/// The client reopens the connection behind its callers and says nothing when
/// the handshake fails — a renewed certificate Redis no longer presents
/// correctly, say — so each caller's command just fails, and nothing names the
/// cause. And what reaches those callers is the refusal's *text*: `redis` hands
/// every caller waiting on a reopened connection a copy of the error it met, and
/// copies an io error by its message, so rustls's reason is gone by the time a
/// command fails — and only once its retries are spent, seconds after the drop,
/// each caller cut at its budget meanwhile. The first command failing on the
/// socket — the drop itself, a budget waited out on the reopening, the shape a
/// refused handshake leaves — therefore starts one handshake of the
/// connection's own, whose error still carries rustls's reason, and a refusal it
/// meets is reported at `warn` with what to change — once, until a command
/// answers again.
struct TlsRefusals {
    /// The client the connection is opened from, so the diagnosing handshake
    /// is the one the client keeps failing: same address, same material.
    client: redis::Client,
    endpoint: String,
    budget: Duration,
    reported: AtomicBool,
    diagnosing: AtomicBool,
}

impl TlsRefusals {
    fn observe<T>(self: &Arc<Self>, outcome: &Result<T, redis::RedisError>) {
        match outcome {
            Ok(_) => {
                if self.reported.load(Ordering::Relaxed) {
                    self.reported.store(false, Ordering::Relaxed);
                }
            }
            Err(error) if tls::negotiation_failed(error) => self.report(error),
            Err(error) if error.is_io_error() => self.diagnose(),
            Err(_) => {}
        }
    }

    fn report(&self, error: &redis::RedisError) {
        if !self.reported.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: crate::TARGET,
                endpoint = %self.endpoint,
                reason = %tls::remedy(error),
                error = %nest_rs_core::error_message(error),
                "redis refused a reopened tls connection",
            );
        }
    }

    /// One handshake per budget at most, and none while the refusal stands
    /// reported: a Redis that is down fails every command on the socket, and
    /// each would otherwise send it one more connection. It runs beside the
    /// command that asked for it, which has failed already and must not wait on
    /// a second handshake past its budget.
    ///
    /// It carries no unit of work's trace, deliberately: the refusal is the
    /// connection's, met by every holder alike, and the command that happened to
    /// meet it first — a request, a job, a worker's read — is not the
    /// one it belongs to, any more than the boot's connection lines are.
    fn diagnose(self: &Arc<Self>) {
        if self.reported.load(Ordering::Relaxed) || self.diagnosing.swap(true, Ordering::Relaxed) {
            return;
        }
        let refusals = Arc::clone(self);
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            let attempt = tokio::time::timeout(
                refusals.budget,
                refusals
                    .client
                    .get_multiplexed_async_connection_with_config(&connection_config(
                        refusals.budget,
                    )),
            )
            .await;
            if let Ok(Err(error)) = attempt
                && tls::negotiation_failed(&error)
            {
                refusals.report(&error);
            }
            tokio::time::sleep_until(started + refusals.budget).await;
            refusals.diagnosing.store(false, Ordering::Relaxed);
        });
    }
}

/// Backoff before the first retry; doubles up to [`MAX_RETRY_BACKOFF`] and is
/// always clamped to what is left of the budget.
const FIRST_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Ceiling for the doubling backoff — a whole boot budget must still fit
/// several attempts, each of which gets its own `warn`. The connection's own
/// reconnect backoff takes the same ceiling.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// The shortest the socket waits before it probes, or before it gives up on
/// what it sent: the kernel counts the first in whole seconds and refuses zero.
const LIVENESS_FLOOR: Duration = Duration::from_secs(1);

/// How much each reconnect attempt's wait grows over the last: the boot's
/// doubling, so a Redis back after a restart is reached within the boot's
/// ceiling rather than a growing wait.
const RECONNECT_FACTOR: f32 = 2.0;

impl RedisConnection {
    /// Open the connection to the Redis `config` names and prove it answers,
    /// giving up after [`connect_timeout`](RedisConfig::connect_timeout).
    ///
    /// Each attempt opens a connection and sends it a `PING`: without the proof
    /// a Redis that accepts the dial and answers nothing boots cleanly and fails
    /// on the first job or the first rate-limited request — and an endpoint that
    /// never answers parks the process with an empty log, never healthy and
    /// never crashed. Every attempt is announced on `nest_rs::redis`, and the
    /// budget converts the hang into the boot error `/queue/wiring/` promises.
    ///
    /// The same budget bounds every command a caller waits on afterwards, so a
    /// Redis that stops answering fails its caller within it rather than
    /// holding the caller.
    ///
    /// A `rediss://` URL encrypts every connection and verifies Redis's
    /// certificate for the URL's host — against the authorities of Mozilla's
    /// root program compiled into the client, or those in
    /// [`RedisConfig::tls`] — presenting the client certificate set there, if
    /// any. A certificate refused after the boot, when the connection reopens,
    /// is reported once at `warn` on `nest_rs::redis`, with what to change.
    ///
    /// What fails the same way on every attempt fails at once instead of
    /// spending the budget on retries that cannot succeed, saying what to
    /// change: a budget under the floor its variable is held to; a URL the
    /// client cannot parse, or one asking for RESP3; TLS settings it will not use — `#insecure`,
    /// material beside a plaintext URL, material no handshake could use; a
    /// handshake that fails the same way every time; and an answer naming the
    /// deployment's own settings — refused credentials, an ACL denying the
    /// proof, a protocol the server does not speak, and a database the server
    /// will not select (an index out of range, an ACL without `+select`, a
    /// server in cluster mode), named by its index. **Every other answer may
    /// clear and is retried within the budget**
    /// — a server loading its dataset, busy running a script, failing over, or
    /// answering with a code this client does not know — as is a refused or
    /// reset TCP connection. A budget spent on answers fails naming the last
    /// one, as a server that is not ready rather than one that cannot be
    /// reached.
    ///
    /// A handshake nobody answers is not among them, because it cannot be told
    /// apart from a network that drops it: a `rediss://` URL pointed at a
    /// plaintext Redis runs out the budget as an unreachable one does.
    pub async fn connect(config: &RedisConfig) -> Result<Self, RedisError> {
        let budget = crate::config::CONNECT_TIMEOUT
            .check(
                RedisConfig::NAMESPACE,
                "RedisConfig::connect_timeout",
                config.connect_timeout,
            )
            .map_err(RedisError::Budget)?;
        let endpoint = address(&config.url);
        let client = client(config, &endpoint, budget)?;
        // The budget was held to its range above, so an hour at most: no clock
        // overflows adding it.
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
            match tokio::time::timeout(remaining, prove(&client, budget)).await {
                Ok(Ok(manager)) => {
                    if attempts > 1 {
                        tracing::info!(
                            target: crate::TARGET,
                            endpoint = %endpoint,
                            attempts,
                            "connected to redis after retrying",
                        );
                    }
                    let refusals = matches!(
                        client.get_connection_info().addr(),
                        ConnectionAddr::TcpTls { .. }
                    )
                    .then(|| {
                        Arc::new(TlsRefusals {
                            client: client.clone(),
                            endpoint: endpoint.clone(),
                            budget,
                            reported: AtomicBool::new(false),
                            diagnosing: AtomicBool::new(false),
                        })
                    });
                    return Ok(Self {
                        kept: Kept::new(manager, client, endpoint, budget, refusals),
                        budget,
                    });
                }
                // The budget elapsed mid-attempt: one final warn so a hung DNS
                // or a black-holed port is as legible as a refused connection.
                Err(_elapsed) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        endpoint = %endpoint,
                        attempt = attempts,
                        timeout_secs = budget.as_secs_f64(),
                        "redis connect timed out",
                    );
                    break;
                }
                Ok(Err(source)) if tls::negotiation_failed(&source) => {
                    return Err(RedisError::TlsRefused {
                        reason: tls::remedy(&source),
                        endpoint,
                        source: Some(source),
                    });
                }
                Ok(Err(source)) if database_refused(&source) => {
                    return Err(RedisError::DatabaseRefused {
                        endpoint,
                        database: client.get_connection_info().redis_settings().db(),
                        source,
                    });
                }
                Ok(Err(source)) if refused(&source) => {
                    return Err(RedisError::Refused { endpoint, source });
                }
                Ok(Err(error)) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        endpoint = %endpoint,
                        attempt = attempts,
                        error = %nest_rs_core::error_message(&error),
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

        Err(match last_error {
            Some(source) if source.code().is_some() => RedisError::Unready {
                endpoint,
                budget,
                attempts,
                source,
            },
            source => RedisError::Unreachable {
                endpoint,
                budget,
                attempts,
                source,
            },
        })
    }
}

impl RedisConnection {
    /// A connection of its own to the same Redis, opened from the same client —
    /// for a command that blocks, which on the shared socket would stall every
    /// other caller. Its commands keep this one's budget until
    /// [`with_budget`](Self::with_budget) widens it.
    pub(crate) async fn dedicated(&self) -> Result<Self, redis::RedisError> {
        let kept = &self.kept;
        let manager = bounded(
            kept.budget,
            ConnectionManager::new_with_config(kept.client.clone(), manager_config(kept.budget)),
        )
        .await??;
        Ok(Self {
            kept: Kept::new(
                manager,
                kept.client.clone(),
                kept.endpoint.clone(),
                kept.budget,
                kept.refusals.clone(),
            ),
            budget: self.budget,
        })
    }

    /// This connection, each command waiting `budget` for its answer — a
    /// blocking command's own wait, plus the budget.
    pub(crate) fn with_budget(&self, budget: Duration) -> Self {
        Self {
            budget,
            ..self.clone()
        }
    }

    /// How long a command on this handle waits for its answer.
    pub(crate) fn budget(&self) -> Duration {
        self.budget
    }

    /// The connection's [`Budget`](nest_rs_core::Budget), declared by
    /// [`RedisModule`](crate::RedisModule) and by each binding beside its net,
    /// so a connection seeded without the module is held too.
    pub(crate) fn declared_budget() -> nest_rs_core::Budget {
        nest_rs_core::Budget::of::<Self>(
            "the Redis connection",
            format!(
                "{}, or `RedisConfig::connect_timeout` in code",
                nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
            ),
            |conn| Some(conn.budget()),
        )
    }

    /// Run `script`, loading it again each time Redis answers it holds none, at
    /// most [`SCRIPT_LOADS`] times.
    pub(crate) async fn invoke<T: FromRedisValue>(
        &self,
        script: &ScriptInvocation<'_>,
    ) -> Result<T, redis::RedisError> {
        let mut conn = self.clone();
        let mut loads = 1;
        loop {
            #[expect(
                clippy::disallowed_methods,
                reason = "the one call every script goes through"
            )]
            let outcome = script.invoke_async(&mut conn).await;
            match outcome {
                Err(error)
                    if loads < SCRIPT_LOADS
                        && error.kind() == ErrorKind::Server(ServerErrorKind::NoScript) =>
                {
                    loads += 1;
                }
                outcome => return outcome,
            }
        }
    }
}

impl ConnectionLike for RedisConnection {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        let budget = self.budget;
        let kept = &self.kept;
        Box::pin(async move {
            let (opening, mut manager) = kept.manager();
            let outcome = bounded(budget, manager.send_packed_command(cmd)).await?;
            kept.observe(opening, &outcome);
            if outcome.as_ref().is_ok_and(read_only) {
                kept.reopen(opening);
            }
            outcome
        })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        pipeline: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        let budget = self.budget;
        let kept = &self.kept;
        Box::pin(async move {
            let (opening, mut manager) = kept.manager();
            let outcome = bounded(
                budget,
                manager.send_packed_commands(pipeline, offset, count),
            )
            .await?;
            kept.observe(opening, &outcome);
            if outcome
                .as_ref()
                .is_ok_and(|replies| replies.iter().any(read_only))
            {
                kept.reopen(opening);
            }
            outcome
        })
    }

    fn get_db(&self) -> i64 {
        self.kept.manager().1.get_db()
    }
}

/// Whether a reply is a server's `READONLY` — `redis` hands a server's error
/// back as a reply, and turns it into an error only once the caller reads it —
/// or holds one, as a transaction's replies do.
fn read_only(reply: &Value) -> bool {
    match reply {
        Value::ServerError(error) => error.code() == "READONLY",
        Value::Array(replies) => replies.iter().any(read_only),
        _ => false,
    }
}

/// `call`, or once `budget` elapses the timeout redis itself reports, so
/// `redis::RedisError::is_timeout` reads both the same way.
async fn bounded<F: Future>(budget: Duration, call: F) -> Result<F::Output, redis::RedisError> {
    #[expect(
        clippy::map_err_ignore,
        reason = "Elapsed carries nothing the timeout's own message does not say"
    )]
    tokio::time::timeout(budget, call).await.map_err(|_| {
        redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "Redis did not answer within {budget:?}, the budget {} sets",
                nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
            ),
        ))
    })
}

/// Whether an attempt failed in a way every attempt will repeat.
///
/// **An answer from Redis repeats only when it names the deployment's own
/// settings**: credentials refused (`WRONGPASS`, `NOAUTH`, or the client's own
/// authentication failure), an ACL denying the proof (`NOPERM`), a protocol it
/// does not speak — and a refused `SELECT`, which [`database_refused`] reads
/// first. Every other answer may clear — `LOADING`, `BUSY` from a script past
/// its threshold, `MASTERDOWN` and `TRYAGAIN` during a failover — and so does a
/// code this client does not know. `redis` marks every unknown code as not worth
/// retrying, which made a Redis running one long script fail the boot in
/// milliseconds with a sentence pointing at the URL; the allow-list is the other
/// way round on purpose, so a code a later server invents is retried within the
/// budget and named as the failure's source, never mistaken for a refusal.
///
/// **What is not an answer** — the client refusing before or around the dial, a
/// socket the process may not open — keeps `redis`'s own verdict.
fn refused(error: &redis::RedisError) -> bool {
    if matches!(
        error.kind(),
        redis::ErrorKind::AuthenticationFailed | redis::ErrorKind::RESP3NotSupported
    ) {
        return true;
    }
    match error.code() {
        Some(code) => matches!(code, "WRONGPASS" | "NOAUTH" | "NOPERM"),
        None => matches!(error.retry_method(), redis::RetryMethod::NoRetry),
    }
}

/// The sentence `redis` reports a refused `SELECT` under, whatever Redis
/// answered: the server's code is dropped and its text kept as the detail. The
/// e2e suite holds it to the client this crate links, against a live server.
const SELECT_REFUSED: &str = "Redis server refused to switch database";

/// Whether Redis refused the `SELECT` of the URL's database for a reason every
/// attempt would repeat.
///
/// The client drops the server's code from a refused `SELECT`, so the code
/// allow-list [`refused`] reads cannot see it: an ACL user without `+select`
/// (`NOPERM`), a server in cluster mode, an index out of range — all `ERR`
/// here — were retried for the whole budget and reported as a Redis that was
/// "not ready", told to widen the budget "if it clears on its own". It never
/// does. So it is read the other way round: `SELECT` runs while a server loads
/// its dataset and on a stale replica (its command flags admit both), and the
/// one transient answer it can meet is a server busy running a script or a
/// module command — retried; anything else repeats, and fails at once.
fn database_refused(error: &redis::RedisError) -> bool {
    error.kind() == redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError)
        && error.to_string().starts_with(SELECT_REFUSED)
        && !error.detail().is_some_and(|detail| {
            let detail = detail.to_ascii_lowercase();
            detail.contains("busy") || detail.contains("loading the dataset")
        })
}

/// The client the connection is opened from, and reopened from, so the URL's
/// TLS and the material beside it reach every connection it opens. Nothing is
/// dialled: each refusal here is one every attempt would repeat.
fn client(
    config: &RedisConfig,
    endpoint: &str,
    budget: Duration,
) -> Result<redis::Client, RedisError> {
    let invalid_url = |source: redis::RedisError| RedisError::InvalidUrl {
        endpoint: endpoint.to_owned(),
        source,
    };
    let info = config
        .url
        .as_str()
        .into_connection_info()
        .map_err(invalid_url)?;
    if info.redis_settings().protocol() != redis::ProtocolVersion::RESP2 {
        return Err(RedisError::UnsupportedProtocol {
            endpoint: endpoint.to_owned(),
        });
    }
    // Every connection the client opens — the proof, the kept one, one it
    // reopens, a blocking read's — learns through its socket that Redis is gone,
    // and sends no `CLIENT SETINFO`, which an ACL user confined to a binding's
    // commands is refused.
    let redis = info.redis_settings().clone().set_skip_set_lib_name();
    let info = info
        .set_tcp_settings(socket(budget))
        .set_redis_settings(redis);
    let ConnectionAddr::TcpTls { host, insecure, .. } = info.addr() else {
        if config.tls.is_set() {
            return Err(RedisError::PlaintextUrl {
                endpoint: endpoint.to_owned(),
            });
        }
        return redis::Client::open(info).map_err(invalid_url);
    };
    if *insecure {
        return Err(RedisError::UnverifiedTls {
            endpoint: endpoint.to_owned(),
        });
    }
    // The name the certificate is verified for, parsed as the handshake parses
    // it: a host no certificate can carry is the URL's fault, on every attempt.
    rustls::pki_types::ServerName::try_from(host.as_str())
        .map_err(|error| invalid_url(error.into()))?;
    config
        .tls
        .check(&tls::crypto_provider())
        .map_err(|reason| RedisError::TlsRefused {
            endpoint: endpoint.to_owned(),
            reason,
            source: None,
        })?;
    redis::Client::build_with_tls(info, config.tls.certificates()).map_err(|source| {
        RedisError::TlsRefused {
            endpoint: endpoint.to_owned(),
            reason: tls::unusable_material(),
            source: Some(source),
        }
    })
}

/// One boot attempt: a connection of its own, proved with a `PING`, then the
/// connection the app keeps.
///
/// The proof runs on a connection opened once, with no retry, because the
/// connection kept is opened by a client that retries on its own and silently —
/// refused credentials included — which would spend the budget on an answer
/// that cannot change and then report it as the network. The kept connection is
/// opened only once the proof has answered, so its own retries are left the one
/// case they serve: a Redis gone between the two.
///
/// **The proof is closed before the kept connection opens.** Held across it,
/// every attempt needed two of the server's client slots, so a Redis with one
/// left — `maxclients` nearly reached by a leak elsewhere, a shared managed
/// instance — answered the proof, refused the kept connection, and the boot
/// spent its whole budget in the client's silent retries before blaming the
/// network. The slot is released as the closed socket reaches the server, so
/// the kept connection may still meet it taken once: its own `PING` then fails
/// the attempt, and the next lands inside the budget.
async fn prove(
    client: &redis::Client,
    budget: Duration,
) -> Result<ConnectionManager, redis::RedisError> {
    let mut proof = client
        .get_multiplexed_async_connection_with_config(&connection_config(budget))
        .await?;
    redis::cmd("PING").query_async::<()>(&mut proof).await?;
    drop(proof);
    // The kept connection is proved too: one that sends nothing on opening — no
    // `AUTH`, no `SELECT` — meets a refusal Redis writes before it closes, a
    // full `maxclients`, only at its first command. The manager reopens it on
    // its own, so the proof is asked again of the same manager, within the
    // attempt: a manager dropped mid-reopen would still take a slot.
    let mut kept =
        ConnectionManager::new_with_config(client.clone(), manager_config(budget)).await?;
    let mut wait = FIRST_RETRY_BACKOFF;
    loop {
        match redis::cmd("PING").query_async::<()>(&mut kept).await {
            Ok(()) => return Ok(kept),
            Err(error) if refused(&error) || tls::negotiation_failed(&error) => return Err(error),
            Err(_) => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(MAX_RETRY_BACKOFF);
            }
        }
    }
}

/// A connection opened once, with no reopening: its dial bounded by the budget,
/// and no reply timeout of the client's — the caller bounds what it waits on.
fn connection_config(budget: Duration) -> AsyncConnectionConfig {
    AsyncConnectionConfig::new()
        .set_connection_timeout(Some(budget))
        .set_response_timeout(None)
}

/// How the kept connection reopens after Redis drops it: each attempt bounded
/// by the budget, as the boot's are, and the backoff between attempts doubling
/// to the boot's ceiling rather than `redis`'s hundredfold. The reply timeout
/// stays off, because [`RedisConnection`] bounds every command itself — the
/// wait for a reopened connection included, which that timeout would miss —
/// and a blocking command's wait is its own.
fn manager_config(budget: Duration) -> ConnectionManagerConfig {
    ConnectionManagerConfig::new()
        .set_connection_timeout(Some(budget))
        .set_response_timeout(None)
        .set_min_delay(FIRST_RETRY_BACKOFF)
        .set_exponent_base(RECONNECT_FACTOR)
        .set_max_delay(MAX_RETRY_BACKOFF)
}

/// How every socket the client opens is set: a command is written whole and
/// waits on its answer, so Nagle's algorithm would only hold it back behind the
/// previous one's acknowledgement (`TCP_NODELAY`, which `redis-rs` leaves off);
/// and its [`liveness`].
fn socket(budget: Duration) -> TcpSettings {
    liveness(budget).set_nodelay(true)
}

/// How the socket learns that Redis is gone rather than slow, so a connection
/// the client holds open is reopened when it is: keepalive probes once the socket has been idle for the budget, and on
/// Linux a `TCP_USER_TIMEOUT` of the budget, which drops the socket when what
/// was sent has gone unacknowledged that long — a network that swallows
/// packets, a host that vanished. A Redis that is only slow — paused, forking,
/// busy with another client's script — still acknowledges every byte, so
/// neither fires, and the answer is received when it comes.
///
/// The kernel reads the idle time in whole seconds, and refuses zero — which a
/// budget under a second would round to, failing every dial — so both are a
/// second at the least: under that, a retransmitted packet would drop a socket
/// whose answers a command is waiting for. And it refuses an idle time past
/// [`KEEPALIVE_IDLE_LIMIT`] and a user timeout past [`USER_TIMEOUT_LIMIT`] with
/// `EINVAL`, on every dial, so the budget's ceiling sits under both — asserted
/// below at compile time rather than clamped here, since a budget the boot
/// accepts can never reach either.
fn liveness(budget: Duration) -> TcpSettings {
    let silence = budget.max(LIVENESS_FLOOR);
    let settings = TcpSettings::default().set_keepalive(TcpKeepalive::new().with_time(silence));
    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
    let settings = settings.set_user_timeout(silence);
    settings
}

/// Linux's `MAX_TCP_KEEPIDLE` (`include/net/tcp.h`): the longest idle time
/// `TCP_KEEPIDLE` accepts, in seconds.
const KEEPALIVE_IDLE_LIMIT: Duration = Duration::from_secs(32_767);

/// The longest `TCP_USER_TIMEOUT` Linux accepts: an `int` of milliseconds.
const USER_TIMEOUT_LIMIT: Duration = Duration::from_millis(i32::MAX as u64);

const _: () = {
    let most = crate::config::CONNECT_TIMEOUT
        .unit()
        .duration(crate::config::CONNECT_TIMEOUT.most().count);
    assert!(
        most.as_secs() <= KEEPALIVE_IDLE_LIMIT.as_secs()
            && most.as_millis() <= USER_TIMEOUT_LIMIT.as_millis(),
        "the connect budget's ceiling sets the socket's liveness, and must stay under what the \
         kernel accepts for both",
    );
};

/// The endpoint the connect diagnostics name, in logs and in the boot error:
/// the address the client parsed from the URL — `host:port`, or a socket path —
/// and never the URL, which routinely embeds a password in its userinfo or its
/// query. The client's own parser decides, so what is shown is what is dialled;
/// a URL it cannot parse is named by its scheme alone, and by nothing at all
/// when what precedes `://` is not a scheme and may be a credential.
fn address(url: &str) -> String {
    match url.into_connection_info() {
        Ok(info) => info.addr().to_string(),
        Err(_) => match url.split_once("://") {
            Some((scheme, _)) if is_scheme(scheme) => format!("{scheme}://<unparseable>"),
            _ => "<unparseable>".to_owned(),
        },
    }
}

/// RFC 3986 §3.1: a letter, then letters, digits, `+`, `-` or `.`.
fn is_scheme(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    use super::*;
    use crate::RedisTlsIdentity;

    const AUTHORITY: &[u8] = include_bytes!("../tests/harness/fixtures/tls_ca.pem");
    const SERVER_CERT: &[u8] = include_bytes!("../tests/harness/fixtures/tls_server.pem");
    const SERVER_KEY: &[u8] = include_bytes!("../tests/harness/fixtures/tls_server.key.pem");
    const CLIENT_CERT: &[u8] = include_bytes!("../tests/harness/fixtures/tls_client.pem");

    const REFUSED: &str = "redis refused a reopened tls connection";

    /// `url` under `budget`, everything else at its default.
    fn config(url: &str, budget: Duration) -> RedisConfig {
        RedisConfig {
            url: url.to_owned(),
            connect_timeout: budget,
            ..RedisConfig::default()
        }
    }

    /// A TLS listener presenting the fixtures' server certificate — which the
    /// test authority signed, and nothing a client trusts by default — and
    /// answering nothing past the handshake.
    async fn tls_listener() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let chain = CertificateDer::pem_slice_iter(SERVER_CERT)
            .collect::<Result<Vec<_>, _>>()
            .expect("the fixture certificate parses");
        let key = PrivateKeyDer::from_pem_slice(SERVER_KEY).expect("the fixture key parses");
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("the provider speaks the default protocol versions")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("the fixture certificate and key correspond");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a TLS listener");
        let addr = listener.local_addr().expect("the listener's address");
        let serving = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "the test listener serves whoever connects; a failed handshake is the client's assertion"
                    )]
                    let _ = acceptor.accept(socket).await;
                });
            }
        });
        (addr, serving)
    }

    /// A refusal's record, over a client opened from `url` as the boot opens
    /// it — so rustls has its process-default provider when the record
    /// handshakes.
    fn refusals(url: &str) -> Arc<TlsRefusals> {
        refusals_within(url, Duration::from_secs(2))
    }

    /// [`refusals`] under `budget`.
    fn refusals_within(url: &str, budget: Duration) -> Arc<TlsRefusals> {
        let config = config(url, budget);
        let endpoint = address(url);
        Arc::new(TlsRefusals {
            client: client(&config, &endpoint, config.connect_timeout)
                .expect("the URL opens a client"),
            endpoint,
            budget: config.connect_timeout,
            reported: AtomicBool::new(false),
            diagnosing: AtomicBool::new(false),
        })
    }

    fn handshake_refused() -> Result<(), redis::RedisError> {
        Err(redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
        )))
    }

    fn connection_dropped() -> Result<(), redis::RedisError> {
        Err(redis::RedisError::from(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )))
    }

    /// What `redis` hands the callers of a connection it could not reopen: the
    /// refusal's text, under the io kind rustls's error had.
    fn reopening_refused() -> Result<(), redis::RedisError> {
        Err(redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Reconnecting failed: invalid peer certificate: UnknownIssuer",
        )))
    }

    /// A TLS refusal is reported once, stays reported while commands keep
    /// failing — whatever else fails meanwhile — and is reported again only
    /// after a command answered in between.
    #[tokio::test]
    async fn a_tls_refusal_is_reported_once_until_a_command_answers_again() {
        let logs = nest_rs_testing::LogCapture::install();
        let refusals = refusals("rediss://redis.internal:6380/");

        refusals.observe(&handshake_refused());
        refusals.observe(&connection_dropped());
        refusals.observe(&handshake_refused());
        refusals.observe(&reopening_refused());
        assert_eq!(logs.find(crate::TARGET, REFUSED).len(), 1, "once");
        assert!(
            !refusals.diagnosing.load(Ordering::Relaxed),
            "and a refusal already reported sends no handshake to learn it again",
        );

        refusals.observe(&Ok::<(), redis::RedisError>(()));
        refusals.observe(&handshake_refused());
        let reported = logs.find(crate::TARGET, REFUSED);
        assert_eq!(reported.len(), 2, "and again after a command answered");
        assert_eq!(reported[0].level, "warn");
        assert_eq!(
            reported[0].field("endpoint").as_deref(),
            Some("redis.internal:6380")
        );
        assert!(
            reported[0].field("reason").is_some_and(
                |reason| reason.contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT"))
            ),
            "with the setting that fixes it: {:?}",
            reported[0].fields,
        );
    }

    /// `redis` hands a caller only the text of a refused reopening, so the
    /// refusal is learnt from a handshake of the record's own — which still
    /// carries rustls's reason, so the line says what to change rather than
    /// quoting a message.
    #[tokio::test]
    async fn a_refusal_known_only_by_its_text_is_learnt_from_a_handshake_of_its_own() {
        let logs = nest_rs_testing::LogCapture::install();
        let (addr, serving) = tls_listener().await;
        let refusals = refusals(&format!("rediss://{addr}/"));

        refusals.observe(&reopening_refused());
        refusals.observe(&reopening_refused());
        let deadline = Instant::now() + Duration::from_secs(5);
        while logs.find(crate::TARGET, REFUSED).is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        serving.abort();

        let reported = logs.find(crate::TARGET, REFUSED);
        assert_eq!(reported.len(), 1, "{:#?}", logs.events());
        let reason = reported[0].field("reason").unwrap_or_default();
        assert!(
            reason.contains("does not chain to an authority")
                && reason.contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT")),
            "the handshake's own reason, sending the operator to the authority: {reason}",
        );
    }

    /// An answer is Redis's, and sends no handshake; a failure on the socket —
    /// the drop a refused reopening starts with, a budget waited out on the
    /// reopening — sends one, one per budget at most, and a peer that refuses
    /// the connection rather than its certificate is reported as nothing.
    #[tokio::test]
    async fn a_failure_on_the_socket_sends_one_handshake_per_budget_and_an_answer_none() {
        let logs = nest_rs_testing::LogCapture::install();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a listener");
        let addr = listener.local_addr().expect("the listener's address");
        let budget = Duration::from_millis(300);
        let refusals = refusals_within(&format!("rediss://{addr}/"), budget);

        refusals.observe(&Err::<(), _>(answer("ERR")));
        assert!(!refusals.diagnosing.load(Ordering::Relaxed), "an answer");

        let timed_out = || {
            Err::<(), _>(redis::RedisError::from(std::io::Error::from(
                std::io::ErrorKind::TimedOut,
            )))
        };
        let accepted = |listener: tokio::net::TcpListener| async move {
            let mut count = 0;
            while let Ok(Ok((socket, _))) =
                tokio::time::timeout(Duration::from_millis(100), listener.accept()).await
            {
                drop(socket);
                count += 1;
            }
            (count, listener)
        };
        refusals.observe(&connection_dropped());
        refusals.observe(&timed_out());
        refusals.observe(&connection_dropped());
        let (count, listener) = accepted(listener).await;
        assert_eq!(count, 1, "one handshake for the three failures");

        tokio::time::sleep(budget).await;
        refusals.observe(&timed_out());
        let (count, _) = accepted(listener).await;
        assert_eq!(count, 1, "and one more once the budget has passed");
        assert!(logs.find(crate::TARGET, REFUSED).is_empty());
    }

    /// An answer as Redis sends it, parsed as the client parses one.
    fn answer(line: &str) -> redis::RedisError {
        let value = redis::parse_redis_value(format!("-{line}\r\n").as_bytes())
            .expect("a RESP error parses");
        let redis::Value::ServerError(error) = value else {
            panic!("a RESP error parses as a server error");
        };
        error.into()
    }

    /// Only an answer naming the deployment's own settings is a refusal; every
    /// other answer — the transient ones `redis` knows, and every code it does
    /// not — is retried within the budget. Every retry costs the boot a backoff
    /// and the operator a line telling them to widen the budget, so what repeats
    /// must not get one; but `BUSY` failed the boot in milliseconds, sending the
    /// operator to the URL, so what may clear must.
    #[test]
    fn only_an_answer_naming_the_deployments_settings_refuses_the_boot() {
        for refusing in [
            redis::RedisError::from((
                redis::ErrorKind::AuthenticationFailed,
                "Password authentication failed",
            )),
            answer("WRONGPASS invalid username-password pair or user is disabled."),
            answer("NOAUTH Authentication required."),
            answer("NOPERM User alice has no permissions to run the 'ping' command"),
            redis::RedisError::from((
                redis::ErrorKind::RESP3NotSupported,
                "Redis Server doesn't support HELLO command therefore resp3 cannot be used",
            )),
            redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        ] {
            assert!(refused(&refusing), "{refusing:?} repeats on every attempt");
        }
        for clearing in [
            answer(
                "BUSY Redis is busy running a script. You can only call SCRIPT KILL or SHUTDOWN NOSAVE.",
            ),
            answer("LOADING Redis is loading the dataset in memory"),
            answer(
                "MASTERDOWN Link with MASTER is down and replica-serve-stale-data is set to 'no'.",
            ),
            answer("TRYAGAIN Multiple keys request during rehashing of slot"),
            answer("SOMEDAYCODE a code this client has never heard of"),
            answer("ERR max number of clients reached"),
            redis::RedisError::from((
                redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
                "Redis server refused to switch database",
                "Redis is busy running a script. You can only call SCRIPT KILL or SHUTDOWN NOSAVE."
                    .to_owned(),
            )),
            redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
        ] {
            assert!(
                !refused(&clearing),
                "{clearing:?} may clear, so it is retried"
            );
        }
    }

    /// A refused `SELECT` as the client reports it: its own sentence, the
    /// server's code dropped, the server's text as the detail.
    fn select_refused(detail: &str) -> redis::RedisError {
        redis::RedisError::from((
            redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError),
            SELECT_REFUSED,
            detail.to_owned(),
        ))
    }

    /// config-1r2: the client drops a refused `SELECT`'s code, so an ACL user
    /// without `+select` and a server in cluster mode read as an unknown `ERR`,
    /// were retried for the whole budget and reported as "not ready … widen the
    /// budget if it clears on its own". Every `SELECT` refusal repeats but a
    /// server busy running a script, so every other one fails at once.
    #[test]
    fn a_refused_select_repeats_unless_the_server_is_busy() {
        for repeating in [
            "DB index is out of range",
            "invalid DB index",
            "User alice has no permissions to run the 'select' command",
            "SELECT is not allowed in cluster mode",
            "an answer no server gives today",
        ] {
            assert!(
                database_refused(&select_refused(repeating)),
                "{repeating} repeats on every attempt"
            );
        }
        for clearing in [
            "Redis is busy running a script. You can only call SCRIPT KILL or SHUTDOWN NOSAVE.",
            "Valkey is busy running a module command.",
            "Redis is loading the dataset in memory",
        ] {
            let error = select_refused(clearing);
            assert!(!database_refused(&error), "{clearing} may clear");
            assert!(!refused(&error), "{clearing} is retried within the budget");
        }
        assert!(
            !database_refused(&answer("ERR SELECT is not allowed in cluster mode")),
            "only the client's `SELECT` refusal is read as one"
        );
    }

    /// A zero budget handed to `connect` directly — no config read held it to
    /// the variable's floor — is refused before anything is dialled, naming the
    /// field, instead of giving up after no attempt and blaming the URL.
    #[tokio::test]
    async fn a_zero_budget_built_in_code_is_refused_before_anything_is_dialled() {
        let Err(error) =
            RedisConnection::connect(&config("redis://127.0.0.1:9/", Duration::ZERO)).await
        else {
            panic!("a zero budget must not connect")
        };
        assert!(matches!(error, RedisError::Budget(_)), "{error}");
        let text = error.to_string();
        assert!(
            text.contains(
                "`RedisConfig::connect_timeout` set in code is 0ns, and it must be above zero"
            ) && text.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
            "{text}",
        );
    }

    /// Skipping certificate verification is refused before anything is
    /// dialled, naming the setting that trusts a private authority instead.
    #[tokio::test]
    async fn a_url_asking_to_skip_certificate_verification_is_refused() {
        let Err(error) = RedisConnection::connect(&config(
            "rediss://127.0.0.1:9/#insecure",
            Duration::from_secs(5),
        ))
        .await
        else {
            panic!("`#insecure` must not connect")
        };
        assert!(matches!(error, RedisError::UnverifiedTls { .. }), "{error}");
        assert!(
            error
                .to_string()
                .contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT")),
            "the refusal names the setting to use instead: {error}",
        );
    }

    /// A URL asking for RESP3 is refused before anything is dialled: every
    /// binding reads Redis's RESP2 replies, and over RESP3 the worker's read
    /// failed on every receive.
    #[tokio::test]
    async fn a_url_asking_for_resp3_is_refused_at_once() {
        let started = Instant::now();
        let Err(error) = RedisConnection::connect(&config(
            "redis://127.0.0.1:9/?protocol=resp3",
            Duration::from_secs(5),
        ))
        .await
        else {
            panic!("RESP3 must not connect")
        };
        assert!(started.elapsed() < Duration::from_secs(1), "at once");
        assert!(
            matches!(error, RedisError::UnsupportedProtocol { .. }),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains(&nest_rs_config::var_name("redis", "URL")),
            "the refusal names the setting to change: {error}",
        );
    }

    /// A `rediss://` host no certificate can name is the URL's fault on every
    /// attempt, so it fails at once as one — not as Redis refusing a connection
    /// nothing opened.
    #[tokio::test]
    async fn a_tls_host_no_certificate_can_name_is_an_invalid_url() {
        let started = Instant::now();
        let Err(error) =
            RedisConnection::connect(&config("rediss://-leading:9/", Duration::from_secs(5))).await
        else {
            panic!("a host no certificate can name must not connect")
        };
        assert!(matches!(error, RedisError::InvalidUrl { .. }), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "nothing was dialled, took {:?}",
            started.elapsed(),
        );
    }

    /// TLS material beside a URL that would not use it is refused, over TCP and
    /// over a socket alike: whoever configured it expects it to be used.
    #[tokio::test]
    async fn tls_material_beside_a_plaintext_url_is_refused() {
        for url in [
            "redis://127.0.0.1:9/",
            "redis+unix:///tmp/nest-rs-redis-absent.sock",
        ] {
            let mut pinned = config(url, Duration::from_secs(5));
            pinned.tls.ca_cert = Some(AUTHORITY.to_vec());
            let Err(error) = RedisConnection::connect(&pinned).await else {
                panic!("{url} must not connect in plaintext beside TLS material")
            };
            assert!(
                matches!(error, RedisError::PlaintextUrl { .. }),
                "{url}: {error}"
            );
        }
    }

    /// Material no handshake could use is refused before anything is dialled —
    /// every connection would otherwise fail on it — and the cause names the
    /// variable holding it.
    #[tokio::test]
    async fn tls_material_no_handshake_could_use_fails_before_anything_is_dialled() {
        let mut trusting_nothing = config("rediss://127.0.0.1:9/", Duration::from_secs(5));
        trusting_nothing.tls.ca_cert = Some(b"no certificate in here".to_vec());
        let mut mismatched = config("rediss://127.0.0.1:9/", Duration::from_secs(5));
        mismatched.tls.identity = Some(RedisTlsIdentity {
            cert: CLIENT_CERT.to_vec(),
            key: SERVER_KEY.to_vec(),
        });

        for (pinned, variable) in [(trusting_nothing, "TLS_CA_CERT"), (mismatched, "TLS_KEY")] {
            let started = Instant::now();
            let Err(error) = RedisConnection::connect(&pinned).await else {
                panic!("material {variable} cannot use must not connect")
            };
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "nothing was dialled, took {:?}",
                started.elapsed(),
            );
            assert!(matches!(error, RedisError::TlsRefused { .. }), "{error}");
            let rendered = error.to_string();
            assert!(
                rendered.contains(&nest_rs_config::var_name("redis", variable)),
                "the refusal names {variable}: {rendered}",
            );
        }
    }

    /// A certificate the client does not accept fails the boot at once, with
    /// the setting that fixes it: the same certificate is refused on every
    /// attempt, so retrying it would only spend the budget.
    #[tokio::test]
    async fn a_certificate_the_client_does_not_accept_fails_the_boot_at_once() {
        let (addr, serving) = tls_listener().await;
        let started = Instant::now();
        let outcome = RedisConnection::connect(&config(
            &format!("rediss://{addr}/"),
            Duration::from_secs(5),
        ))
        .await;
        serving.abort();
        let Err(error) = outcome else {
            panic!("a certificate no trusted authority signed must not connect")
        };
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a refused certificate spends none of the budget, took {:?}",
            started.elapsed(),
        );
        assert!(matches!(error, RedisError::TlsRefused { .. }), "{error}");
        assert!(
            error
                .to_string()
                .contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT")),
            "the refusal names the authority's setting: {error}",
        );
    }

    /// The endpoint is what the client dials. Every shape that once reached a
    /// boot error or a `warn` with its password — a password holding `/` or
    /// `?`, a socket's `?pass=`, a URL with no `//`, a URL with no scheme at
    /// all — shows none, whether the client parses it or not.
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
            "s3cr3t@redis:6379",
            "alice:s3cr3t@redis:6379/0",
            "s3cr3t@redis://redis:6379",
        ] {
            let shown = address(url);
            assert!(!shown.contains("s3cr3t"), "{url} showed {shown}");
        }
    }

    /// config-6r2: a budget past the kernel's keepalive limit passed every check
    /// and then failed every dial with `EINVAL`, warning "redis unreachable"
    /// against a Redis that answered, for the whole budget. Past the ceiling it
    /// is refused before anything is dialled, from code and from the variable,
    /// naming the variable — the top of `u64` included, which no clock holds.
    #[tokio::test]
    async fn a_budget_past_the_ceiling_is_refused_before_any_dial() {
        for budget in [
            Duration::from_secs(60 * 60 + 1),
            Duration::from_secs(32_768),
            Duration::MAX,
        ] {
            let attempt = tokio::time::timeout(
                Duration::from_secs(2),
                RedisConnection::connect(&config("redis://127.0.0.1:1/", budget)),
            )
            .await
            .expect("refused at once rather than dialled");
            let Err(RedisError::Budget(refused)) = attempt else {
                panic!("{budget:?} must be refused as a budget");
            };
            let text = refused.to_string();
            assert!(
                text.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"))
                    && text.contains("it must be at most"),
                "{text}",
            );
        }
        let from_env = <RedisConfig as nest_rs_config::Config>::from_env(
            &nest_rs_config::ConfigService::with_vars("redis", [("CONNECT_TIMEOUT_SECS", "32768")]),
            RedisConfig::default(),
        )
        .expect_err("past the ceiling");
        assert!(
            from_env
                .to_string()
                .contains("must be at most 3600 seconds"),
            "{from_env}"
        );
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
        let Err(err) = RedisConnection::connect(&config(
            "redis://alice:s3cr3t@127.0.0.1:9/0",
            Duration::from_millis(700),
        ))
        .await
        else {
            panic!("an unreachable endpoint must not connect, and must not hang")
        };

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the connect budget must bound the wait, took {:?}",
            started.elapsed(),
        );
        assert!(matches!(err, RedisError::Unreachable { .. }), "{err}");
        let rendered = err.to_string();
        assert!(
            rendered.contains("127.0.0.1:9"),
            "the error names the endpoint: {rendered}",
        );
        assert!(
            rendered.contains("700ms"),
            "the error names the budget as it was set, not rounded to whole seconds: {rendered}",
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
        let Err(err) = RedisConnection::connect(&config(
            "redis://user:secret@[::bad-host/",
            Duration::from_secs(5),
        ))
        .await
        else {
            panic!("a URL the client rejects must fail the boot")
        };

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a deterministic failure spends none of the budget, took {:?}",
            started.elapsed(),
        );
        assert!(matches!(err, RedisError::InvalidUrl { .. }), "{err}");
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

        let outcome = RedisConnection::connect(&config(
            &format!("redis://{addr}/"),
            Duration::from_millis(300),
        ))
        .await;
        silent.abort();
        let Err(err) = outcome else {
            panic!("an endpoint that never answers fails the boot rather than parking it")
        };
        assert!(matches!(err, RedisError::Unreachable { .. }), "{err}");

        let expired = logs
            .find(crate::TARGET, "redis connect timed out")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("the expiry is its own event: {:#?}", logs.events()));
        assert_eq!(expired.level, "warn");
        assert_eq!(
            expired.field("timeout_secs").as_deref(),
            Some("0.3"),
            "the event names the budget that elapsed, unrounded: {:?}",
            expired.fields,
        );
    }

    /// A budget under a second still opens a socket: the keepalive's idle time,
    /// which the kernel counts in whole seconds and refuses at zero, is a
    /// second at the least — at zero every dial failed and the boot ran out its
    /// budget retrying it — and so is the time the socket gives what it sent.
    #[test]
    fn the_sockets_liveness_is_a_whole_second_at_the_least() {
        for (budget, shown) in [
            (Duration::from_millis(300), "1s"),
            (Duration::from_millis(999), "1s"),
            (Duration::from_secs(10), "10s"),
        ] {
            let settings = format!("{:?}", liveness(budget));
            assert!(
                settings.contains(&format!("time: Some({shown})")),
                "{budget:?}: {settings}"
            );
            #[cfg(target_os = "linux")]
            assert!(
                settings.contains(&format!("user_timeout: Some({shown})")),
                "{budget:?}: {settings}"
            );
        }
    }

    /// A command leaves the socket when it is written, never held behind the
    /// previous one's acknowledgement — and the socket keeps its liveness.
    #[test]
    fn every_socket_sends_a_command_when_it_is_written() {
        let settings = socket(Duration::from_secs(10));
        assert!(settings.nodelay(), "{settings:?}");
        assert!(settings.keepalive().is_some(), "{settings:?}");
    }

    /// A command that never answers is failed at the budget, as a timeout the
    /// caller can tell apart from a refusal, naming the knob that sets it.
    #[tokio::test]
    async fn a_command_that_never_answers_fails_at_the_budget_as_a_timeout() {
        let budget = Duration::from_millis(150);
        let started = Instant::now();
        let error = bounded(budget, std::future::pending::<()>())
            .await
            .expect_err("a call that never answers is ended by the budget");

        assert!(error.is_timeout(), "it fails as a timeout: {error}");
        assert!(
            started.elapsed() < budget * 4,
            "and at the budget, took {:?}",
            started.elapsed(),
        );
        assert!(
            error
                .to_string()
                .contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
            "naming the knob that sets it: {error}",
        );
    }
}
