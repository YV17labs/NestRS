//! [`RedisConnection`] — the one connection every binding in this crate shares
//! to reach Redis, opened once by [`RedisModule`](crate::RedisModule) over the
//! link the topology its URL declares needs: one server
//! ([`StandaloneLink`]), the primary the sentinels name ([`SentinelLink`]), or
//! a Cluster's nodes ([`ClusterLink`]).
//!
//! **Every command a caller waits on answers or fails within the connect
//! budget**, on every topology — the wait for a reopened connection, for a
//! primary the sentinels name again, or for a redirection included, which a
//! reply timeout inside the client would never cover. A blocking command gets a
//! connection of its own ([`RedisConnection::dedicated`]).

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nest_rs_config::Namespaced;
use redis::AsyncConnectionConfig;
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster_routing::Slot;
use redis::io::tcp::TcpSettings;
use redis::io::tcp::socket2::TcpKeepalive;
use redis::{
    Cmd, ConnectionAddr, ConnectionInfo, ErrorKind, FromRedisValue, IntoConnectionInfo, Pipeline,
    RedisConnectionInfo, RedisFuture, ServerErrorKind, Value,
};

use crate::cluster::{ClusterLink, SlotLink};
use crate::config::CONNECT_TIMEOUT;
use crate::error::RedisError;
use crate::script::{Invocation, RedisScript};
use crate::sentinel::SentinelLink;
use crate::standalone::StandaloneLink;
use crate::url::{NodeAddr, RedisUrl};
use crate::{RedisConfig, tls};

/// The sentence every binding's boot error appends when the connection is
/// missing.
pub(crate) const CONNECTION_REMEDY: &str = "RedisConnection is not registered — import \
     RedisModule::for_root(None), which opens the one Redis connection every Redis binding shares";

/// How many times one call loads its script, at most: a `SCRIPT FLUSH` or a
/// failover landing between a load and the call that follows it would
/// otherwise fail the call.
const SCRIPT_LOADS: u32 = 3;

/// The app's shared Redis connection, and a connection in its own right: it
/// implements [`ConnectionLike`], so a clone runs any `redis` command, script or
/// pipeline — pass `&mut` the clone to `query_async` or `invoke_async`. A clone
/// shares the one underlying link.
///
/// A command answers or fails within the connect budget; when the budget is
/// what ends it, the error is a timeout (`redis::RedisError::is_timeout`). A
/// timeout says the answer did not arrive in time, never that the command did
/// not run: a command Redis was holding still runs once Redis answers again,
/// and its late reply goes to nobody while the next command gets its own.
///
/// Every holder multiplexes over the same sockets and sessions — the queue and
/// the rate limiter included. Redis answers one connection's commands in order,
/// so a **blocking command** (`BLPOP`, `WAIT`, a `SUBSCRIBE`) stalls every holder
/// for as long as it blocks, and fails at the budget besides; and a **command
/// that changes the session** (`SELECT`, `WATCH`, a `MULTI` sent outside an
/// atomic pipeline, `CLIENT SETNAME`, `AUTH`) changes it for every holder. Open a
/// `redis::Client` of your own for those. Non-blocking, atomic operations (a
/// `Script`, `INCR`, `GET`/`SET`, an atomic pipeline) are the intended traffic;
/// on a Cluster, a script or a pipeline names keys of one hash slot.
#[derive(Clone)]
pub struct RedisConnection {
    /// The link every clone shares.
    link: Link,
    /// How long a command on this handle waits for its answer.
    budget: Duration,
}

/// The link a connection reaches Valkey through, one per topology.
#[derive(Clone)]
enum Link {
    Standalone(Arc<StandaloneLink>),
    Sentinel(Arc<SentinelLink>),
    Cluster(Arc<ClusterLink>),
    /// A Cluster's link dedicated to one slot's primary.
    Slot(Arc<SlotLink>),
}

/// Backoff before the first retry; doubles up to [`MAX_RETRY_BACKOFF`] and is
/// always clamped to what is left of the budget.
pub(crate) const FIRST_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Ceiling for the doubling backoff, so a boot budget fits several attempts;
/// every link's reopening backoff takes it too.
pub(crate) const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// The shortest the socket waits before it probes, or before it gives up on
/// what it sent: the kernel counts the first in whole seconds and refuses zero.
const LIVENESS_FLOOR: Duration = Duration::from_secs(1);

/// How much each reconnect attempt's wait grows over the last: the boot's
/// doubling.
pub(crate) const RECONNECT_FACTOR: f32 = 2.0;

impl RedisConnection {
    /// Open the connection to the Redis `config` names and prove it answers,
    /// giving up after [`connect_timeout`](RedisConfig::connect_timeout).
    ///
    /// The URL's scheme declares the topology: `redis://` and `rediss://` one
    /// server; `redis-sentinel://` and `rediss-sentinel://` the primary the
    /// sentinels it lists name under `sentinelServiceName`, asked again after
    /// every failover; `redis-cluster://` and `rediss-cluster://` a Cluster,
    /// whose nodes are read from the ones it lists. Each attempt opens what the
    /// topology needs and proves it, announced on `nest_rs::redis`. The same
    /// budget bounds every command a caller waits on afterwards.
    ///
    /// A scheme ending in `s` encrypts every connection — to the sentinels and
    /// to every node — and verifies each certificate for the host dialled:
    /// against the system's authorities, or those in [`RedisConfig::tls`],
    /// presenting the client certificate set there, if any. A certificate
    /// refused after the boot, when a connection reopens, is reported once at
    /// `warn` on `nest_rs::redis`, with what to change.
    ///
    /// What fails the same way on every attempt fails at once instead of
    /// spending the budget on retries that cannot succeed, saying what to
    /// change: a budget under the floor its variable is held to; a URL the
    /// client cannot parse, one asking for RESP3, one naming a parameter its
    /// topology does not take; TLS settings it will not use — `#insecure`,
    /// material beside a plaintext URL, material no handshake could use; a
    /// handshake that fails the same way every time; a server that is not the
    /// topology the URL declares; and an answer naming the deployment's own
    /// settings — refused credentials, an ACL denying the proof, a protocol the
    /// server does not speak, and a database the server will not select (an
    /// index out of range, an ACL without `+select`), named by its index.
    /// **Every other answer may clear and is retried within the budget** — a
    /// server loading its dataset, busy running a script, failing over, a
    /// Cluster whose slots are not all served, sentinels that do not know the
    /// primary yet, or answering with a code this client does not know — as is
    /// a refused or reset TCP connection. A budget spent on answers fails naming
    /// the last one, as a server that is not ready rather than one that cannot
    /// be reached. A `rediss://` URL pointed at a plaintext Redis runs out the
    /// budget as an unreachable one does.
    pub async fn connect(config: &RedisConfig) -> Result<Self, RedisError> {
        let budget = CONNECT_TIMEOUT
            .check(
                RedisConfig::NAMESPACE,
                CONNECT_TIMEOUT.field(),
                config.connect_timeout,
            )
            .map_err(RedisError::Budget)?;
        let link = match RedisUrl::parse(&config.url)? {
            RedisUrl::Standalone(info) => {
                Link::Standalone(StandaloneLink::connect(info, &config.tls, budget).await?)
            }
            RedisUrl::Sentinel(url) => {
                Link::Sentinel(SentinelLink::connect(url, &config.tls, budget).await?)
            }
            RedisUrl::Cluster(url) => {
                Link::Cluster(ClusterLink::connect(url, &config.tls, budget).await?)
            }
        };
        Ok(Self { link, budget })
    }
}

impl RedisConnection {
    /// A connection of its own to the same Redis, opened from the same settings
    /// — for a command that blocks on `key`, which on the shared link would
    /// stall every other caller — its commands waiting this handle's budget: a
    /// blocking command's own wait plus the budget, set with
    /// [`with_budget`](Self::with_budget) first.
    ///
    /// On a Cluster it reaches the primary serving `key`'s slot alone, and a
    /// `MOVED`, a drop or a timeout fails its command rather than reopening it:
    /// its holder opens another, which asks the Cluster where the slot is now.
    pub(crate) async fn dedicated(&self, key: &str) -> Result<Self, redis::RedisError> {
        let link = match &self.link {
            Link::Standalone(link) => Link::Standalone(link.dedicated().await?),
            Link::Sentinel(link) => Link::Sentinel(link.dedicated().await?),
            Link::Cluster(link) => Link::Slot(link.dedicated(key).await?),
            Link::Slot(link) => Link::Slot(link.dedicated(key).await?),
        };
        Ok(Self {
            link,
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
                "{}, or `{}` in code",
                nest_rs_config::var_name(RedisConfig::NAMESPACE, CONNECT_TIMEOUT.key()),
                CONNECT_TIMEOUT.field(),
            ),
            |conn| Some(conn.budget()),
        )
    }

    /// `builder` with the connection's budget and `port`'s net over it, which
    /// the boot holds the budget under — what every binding handing the
    /// connection to a port declares.
    pub(crate) fn netted(
        builder: nest_rs_core::ContainerBuilder,
        port: &'static str,
        net: Duration,
    ) -> nest_rs_core::ContainerBuilder {
        builder
            .provide_meta(Self::declared_budget())
            .provide_meta(nest_rs_core::Net::over::<Self>(port, net))
    }

    /// The connection [`RedisModule`](crate::RedisModule) opened, for
    /// `binding`'s factory.
    pub(crate) fn of(container: &nest_rs_core::Container, binding: &str) -> anyhow::Result<Self> {
        container
            .get::<Self>()
            .map(|conn| (*conn).clone())
            .ok_or_else(|| anyhow::anyhow!("{binding}: {CONNECTION_REMEDY}"))
    }

    /// Run `invocation`, loading its script on the server that ran it each time
    /// that server answers it holds none, at most [`SCRIPT_LOADS`] times. A call
    /// whose keys sit in two hash slots is refused before it is sent.
    pub(crate) async fn invoke<T: FromRedisValue>(
        &self,
        invocation: &Invocation<'_>,
    ) -> Result<T, redis::RedisError> {
        let slot = invocation.slot()?;
        let eval = invocation.eval_cmd();
        let mut conn = self.clone();
        let mut loads = 0;
        loop {
            match eval.query_async::<T>(&mut conn).await {
                Err(error)
                    if loads < SCRIPT_LOADS
                        && error.kind() == ErrorKind::Server(ServerErrorKind::NoScript) =>
                {
                    self.load_at(invocation.script(), slot).await?;
                    loads += 1;
                }
                outcome => return outcome,
            }
        }
    }

    /// Cache `script` wherever its calls run: the one server, the primary, or
    /// every primary of a Cluster.
    pub(crate) async fn load(&self, script: &RedisScript) -> Result<(), redis::RedisError> {
        self.load_at(script, None).await
    }

    /// Cache `script` on the node serving `slot`, or wherever its calls run
    /// when the call names no key.
    async fn load_at(
        &self,
        script: &RedisScript,
        slot: Option<Slot>,
    ) -> Result<(), redis::RedisError> {
        match &self.link {
            Link::Cluster(link) => link.load(script, slot, self.budget).await,
            Link::Standalone(_) | Link::Sentinel(_) | Link::Slot(_) => {
                script.load_cmd().query_async::<()>(&mut self.clone()).await
            }
        }
    }
}

impl ConnectionLike for RedisConnection {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        let budget = self.budget;
        let link = &self.link;
        Box::pin(async move {
            match link {
                Link::Standalone(link) => link.send(cmd, budget).await,
                Link::Sentinel(link) => link.send(cmd, budget).await,
                Link::Cluster(link) => link.send(cmd, budget).await,
                Link::Slot(link) => link.send(cmd, budget).await,
            }
        })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        pipeline: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        let budget = self.budget;
        let link = &self.link;
        Box::pin(async move {
            match link {
                Link::Standalone(link) => link.send_pipeline(pipeline, offset, count, budget).await,
                Link::Sentinel(link) => link.send_pipeline(pipeline, offset, count, budget).await,
                Link::Cluster(link) => link.send_pipeline(pipeline, offset, count, budget).await,
                Link::Slot(link) => link.send_pipeline(pipeline, offset, count, budget).await,
            }
        })
    }

    fn get_db(&self) -> i64 {
        match &self.link {
            Link::Standalone(link) => link.db(),
            Link::Sentinel(link) => link.db(),
            Link::Cluster(link) => link.db(),
            Link::Slot(link) => link.db(),
        }
    }
}

/// Why one attempt to open a link failed.
pub(crate) enum Attempt<E> {
    /// What every attempt would meet, which fails the boot at once.
    Refused(RedisError),
    /// What may clear, so the next attempt follows within the budget.
    Failed(E),
}

/// How an attempt loop ended without a link.
pub(crate) enum GaveUp<E> {
    /// An attempt met what every attempt would.
    Refused(RedisError),
    /// The budget was spent on attempts that may have cleared.
    Spent { attempts: u32, last: Option<E> },
}

/// Run `attempt` until it opens a link, meets a refusal, or `budget` is spent
/// between failures that may clear, each announced on `nest_rs::redis` naming
/// `endpoint`. Each attempt is handed what is left of the budget.
pub(crate) async fn within_budget<T, E, F, Fut>(
    budget: Duration,
    endpoint: &str,
    mut attempt: F,
) -> Result<T, GaveUp<E>>
where
    F: FnMut(Duration) -> Fut,
    Fut: Future<Output = Result<T, Attempt<E>>>,
    E: std::error::Error + 'static,
{
    // The budget was held to its range, so an hour at most: no clock overflows
    // adding it.
    let deadline = Instant::now() + budget;
    let mut backoff = FIRST_RETRY_BACKOFF;
    let mut attempts = 0u32;
    let mut last = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        attempts += 1;
        match tokio::time::timeout(remaining, attempt(remaining)).await {
            Ok(Ok(opened)) => {
                if attempts > 1 {
                    tracing::info!(
                        target: crate::TARGET,
                        endpoint = %endpoint,
                        attempts,
                        "connected to redis after retrying",
                    );
                }
                return Ok(opened);
            }
            // One final warn, so a hung DNS or a black-holed port is said.
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
            Ok(Err(Attempt::Refused(refusal))) => return Err(GaveUp::Refused(refusal)),
            Ok(Err(Attempt::Failed(error))) => {
                tracing::warn!(
                    target: crate::TARGET,
                    endpoint = %endpoint,
                    attempt = attempts,
                    error = %nest_rs_core::error_message(&error),
                    "redis unreachable — retrying within the connect budget",
                );
                last = Some(error);
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        tokio::time::sleep(backoff.min(left)).await;
        backoff = backoff_after(backoff);
    }
    Err(GaveUp::Spent { attempts, last })
}

/// The boot error of a server that answered or failed on every attempt until
/// the budget was spent: not ready when its last failure was an answer,
/// unreachable otherwise.
pub(crate) fn spent(
    endpoint: String,
    budget: Duration,
    attempts: u32,
    last: Option<redis::RedisError>,
) -> RedisError {
    match last {
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
    }
}

impl GaveUp<redis::RedisError> {
    /// The boot error a spent or refused attempt loop on `endpoint` ends in.
    pub(crate) fn into_error(self, endpoint: &str, budget: Duration) -> RedisError {
        match self {
            Self::Refused(refusal) => refusal,
            Self::Spent { attempts, last } => spent(endpoint.to_owned(), budget, attempts, last),
        }
    }
}

/// The wait after `wait`: doubled, up to [`MAX_RETRY_BACKOFF`].
pub(crate) fn backoff_after(wait: Duration) -> Duration {
    (wait * 2).min(MAX_RETRY_BACKOFF)
}

/// A client of one host a topology names — a sentinel, the primary it names,
/// a Cluster's seed — with `settings`, encrypted when `certificates` are
/// given.
pub(crate) fn node_client(
    node: &NodeAddr,
    settings: RedisConnectionInfo,
    certificates: Option<&redis::TlsCertificates>,
    budget: Duration,
) -> Result<redis::Client, RedisError> {
    let endpoint = node.to_string();
    let info = node_addr(node, certificates.is_some())
        .into_connection_info()
        .map_err(|source| RedisError::InvalidUrl {
            endpoint: endpoint.clone(),
            source,
        })?
        .set_redis_settings(settings);
    open_client(info, certificates, &endpoint, budget)
}

/// `node`'s address, encrypted or not — verified whenever it is.
pub(crate) fn node_addr(node: &NodeAddr, encrypted: bool) -> ConnectionAddr {
    if encrypted {
        ConnectionAddr::TcpTls {
            host: node.host.clone(),
            port: node.port,
            insecure: false,
            tls_params: None,
        }
    } else {
        ConnectionAddr::Tcp(node.host.clone(), node.port)
    }
}

/// The client of `info`, its sockets set for `budget`, encrypted with
/// `certificates` when given. Nothing is dialled.
pub(crate) fn open_client(
    info: ConnectionInfo,
    certificates: Option<&redis::TlsCertificates>,
    endpoint: &str,
    budget: Duration,
) -> Result<redis::Client, RedisError> {
    let info = info.set_tcp_settings(socket(budget));
    match certificates {
        None => redis::Client::open(info).map_err(|source| RedisError::InvalidUrl {
            endpoint: endpoint.to_owned(),
            source,
        }),
        Some(certificates) => {
            redis::Client::build_with_tls(info, certificates.clone()).map_err(|source| {
                RedisError::TlsRefused {
                    endpoint: endpoint.to_owned(),
                    reason: tls::unusable_material(),
                    source: Some(source),
                }
            })
        }
    }
}

/// What one failed dial or proof met: a refusal every attempt would repeat, or
/// a failure that may clear.
pub(crate) fn classify(
    source: redis::RedisError,
    endpoint: &str,
    database: i64,
) -> Attempt<redis::RedisError> {
    if tls::negotiation_failed(&source) {
        return Attempt::Refused(RedisError::TlsRefused {
            reason: tls::remedy(&source),
            endpoint: endpoint.to_owned(),
            source: Some(source),
        });
    }
    if database_refused(&source) {
        return Attempt::Refused(RedisError::DatabaseRefused {
            endpoint: endpoint.to_owned(),
            database,
            source,
        });
    }
    if refused(&source) {
        return Attempt::Refused(RedisError::Refused {
            endpoint: endpoint.to_owned(),
            source,
        });
    }
    Attempt::Failed(source)
}

/// What a command or a pipeline got back.
pub(crate) trait Replies {
    /// Whether a reply is a server's `READONLY` — `redis` hands a server's
    /// error back as a reply, and turns it into an error only once the caller
    /// reads it — or holds one, as a transaction's replies do.
    fn read_only(&self) -> bool;
}

impl Replies for Value {
    fn read_only(&self) -> bool {
        match self {
            Value::ServerError(error) => error.code() == "READONLY",
            Value::Array(replies) => replies.read_only(),
            _ => false,
        }
    }
}

impl Replies for Vec<Value> {
    fn read_only(&self) -> bool {
        self.iter().any(Replies::read_only)
    }
}

/// Whether the server that met `outcome` no longer takes writes: it answered
/// `READONLY`, as a reply or as an error.
pub(crate) fn demoted<T: Replies>(outcome: &Result<T, redis::RedisError>) -> bool {
    match outcome {
        Ok(replies) => replies.read_only(),
        Err(error) => error.code() == Some("READONLY"),
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

/// `call`, a command's own outcome, answered or failed within `budget`.
pub(crate) async fn answered<T, F>(budget: Duration, call: F) -> Result<T, redis::RedisError>
where
    F: Future<Output = Result<T, redis::RedisError>>,
{
    bounded(budget, call).await?
}

/// Whether an attempt failed in a way every attempt will repeat.
///
/// **An answer from Redis repeats only when it names the deployment's own
/// settings**: credentials refused (`WRONGPASS`, `NOAUTH`, or the client's own
/// authentication failure), an ACL denying the proof (`NOPERM`), a protocol it
/// does not speak — and a refused `SELECT`, which [`database_refused`] reads
/// first. Every other code, one this client does not know included, may clear:
/// `redis` marks an unknown code `NoRetry`, so the allow-list is ours.
///
/// **What is not an answer** — the client refusing before or around the dial, a
/// socket the process may not open — keeps `redis`'s own verdict.
pub(crate) fn refused(error: &redis::RedisError) -> bool {
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
/// The client drops the server's code from a refused `SELECT`, so [`refused`]
/// cannot see it; the one transient answer `SELECT` can meet is a server busy
/// running a script or a module command, and anything else fails at once.
pub(crate) fn database_refused(error: &redis::RedisError) -> bool {
    error.kind() == redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError)
        && error.to_string().starts_with(SELECT_REFUSED)
        && !error.detail().is_some_and(|detail| {
            let detail = detail.to_ascii_lowercase();
            detail.contains("busy") || detail.contains("loading the dataset")
        })
}

/// A connection of `client`'s opened once, with no reopening: its dial bounded
/// by `budget`, and no reply timeout of the client's — the caller bounds what
/// it waits on.
pub(crate) async fn dial(
    client: &redis::Client,
    budget: Duration,
) -> Result<MultiplexedConnection, redis::RedisError> {
    let config = AsyncConnectionConfig::new()
        .set_connection_timeout(Some(budget))
        .set_response_timeout(None);
    client
        .get_multiplexed_async_connection_with_config(&config)
        .await
}

/// How every socket a client of this crate opens is set: a command is written
/// whole and waits on its answer, so Nagle's algorithm would only hold it back
/// behind the previous one's acknowledgement (`TCP_NODELAY`, which `redis-rs`
/// leaves off); and its [`liveness`].
pub(crate) fn socket(budget: Duration) -> TcpSettings {
    liveness(budget).set_nodelay(true)
}

/// How the socket learns that Redis is gone rather than slow: keepalive probes
/// once idle for the budget, and on Linux a `TCP_USER_TIMEOUT` of the budget. A
/// Redis that is only slow still acknowledges every byte, so neither fires.
///
/// Both are a second at the least: the kernel counts the idle time in whole
/// seconds and refuses zero. It refuses an idle time past
/// [`KEEPALIVE_IDLE_LIMIT`] and a user timeout past [`USER_TIMEOUT_LIMIT`] with
/// `EINVAL`, so the budget's ceiling sits under both, asserted below.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RedisTlsIdentity;
    use crate::testing::{
        AUTHORITY, CLIENT, SERVER, answer, config, silent_listener, tls_listener,
    };

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
            answer("CLUSTERDOWN The cluster is down"),
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

    #[tokio::test]
    async fn a_url_asking_to_skip_certificate_verification_is_refused() {
        for url in [
            "rediss://127.0.0.1:9/#insecure",
            "rediss-sentinel://127.0.0.1:9?sentinelServiceName=nestrs#insecure",
            "rediss-cluster://127.0.0.1:9#insecure",
        ] {
            let Err(error) = RedisConnection::connect(&config(url, Duration::from_secs(5))).await
            else {
                panic!("`#insecure` must not connect")
            };
            assert!(
                matches!(error, RedisError::UnverifiedTls { .. }),
                "{url}: {error}"
            );
            assert!(
                error
                    .to_string()
                    .contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT")),
                "the refusal names the setting to use instead: {error}",
            );
        }
    }

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

    #[tokio::test]
    async fn a_tls_host_no_certificate_can_name_is_an_invalid_url() {
        for url in [
            "rediss://-leading:9/",
            "rediss-cluster://-leading:9",
            "rediss-sentinel://-leading:9?sentinelServiceName=nestrs",
        ] {
            let started = Instant::now();
            let Err(error) = RedisConnection::connect(&config(url, Duration::from_secs(5))).await
            else {
                panic!("a host no certificate can name must not connect")
            };
            assert!(
                matches!(error, RedisError::InvalidUrl { .. }),
                "{url}: {error}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "nothing was dialled, took {:?}",
                started.elapsed(),
            );
        }
    }

    #[tokio::test]
    async fn tls_material_beside_a_plaintext_url_is_refused() {
        for url in [
            "redis://127.0.0.1:9/",
            "redis+unix:///tmp/nest-rs-redis-absent.sock",
            "redis-sentinel://127.0.0.1:9?sentinelServiceName=nestrs",
            "redis-cluster://127.0.0.1:9",
        ] {
            let mut pinned = config(url, Duration::from_secs(5));
            pinned.tls.ca_cert = Some(AUTHORITY.pem().as_bytes().to_vec());
            let Err(error) = RedisConnection::connect(&pinned).await else {
                panic!("{url} must not connect in plaintext beside TLS material")
            };
            assert!(
                matches!(error, RedisError::PlaintextUrl { .. }),
                "{url}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn tls_material_no_handshake_could_use_fails_before_anything_is_dialled() {
        for url in [
            "rediss://127.0.0.1:9/",
            "rediss-sentinel://127.0.0.1:9?sentinelServiceName=nestrs",
            "rediss-cluster://127.0.0.1:9",
        ] {
            let mut trusting_nothing = config(url, Duration::from_secs(5));
            trusting_nothing.tls.ca_cert = Some(b"no certificate in here".to_vec());
            let mut mismatched = config(url, Duration::from_secs(5));
            mismatched.tls.identity = Some(RedisTlsIdentity {
                cert: CLIENT.cert.as_bytes().to_vec(),
                key: SERVER.key.as_bytes().to_vec(),
            });

            for (pinned, variable) in [(trusting_nothing, "TLS_CA_CERT"), (mismatched, "TLS_KEY")] {
                let started = Instant::now();
                let Err(error) = RedisConnection::connect(&pinned).await else {
                    panic!("material {variable} cannot use must not connect")
                };
                assert!(
                    started.elapsed() < Duration::from_secs(1),
                    "{url}: nothing was dialled, took {:?}",
                    started.elapsed(),
                );
                assert!(
                    matches!(error, RedisError::TlsRefused { .. }),
                    "{url}: {error}"
                );
                let rendered = error.to_string();
                assert!(
                    rendered.contains(&nest_rs_config::var_name("redis", variable)),
                    "{url}: the refusal names {variable}: {rendered}",
                );
            }
        }
    }

    #[tokio::test]
    async fn a_certificate_the_client_does_not_accept_fails_the_boot_at_once() {
        let (addr, serving) = tls_listener().await;
        for url in [
            format!("rediss://{addr}/"),
            format!("rediss-sentinel://{addr}?sentinelServiceName=nestrs"),
            format!("rediss-cluster://{addr}"),
        ] {
            let started = Instant::now();
            let outcome = RedisConnection::connect(&config(&url, Duration::from_secs(5))).await;
            let Err(error) = outcome else {
                panic!("a certificate no trusted authority signed must not connect")
            };
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "{url}: a refused certificate spends none of the budget, took {:?}",
                started.elapsed(),
            );
            assert!(
                matches!(error, RedisError::TlsRefused { .. }),
                "{url}: {error}"
            );
            assert!(
                error
                    .to_string()
                    .contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT")),
                "{url}: the refusal names the authority's setting: {error}",
            );
        }
        serving.abort();
    }

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

    #[tokio::test]
    async fn unreachable_seeds_and_sentinels_fail_naming_every_one() {
        let budget = Duration::from_millis(700);
        let Err(cluster) = RedisConnection::connect(&config(
            "redis-cluster://alice:s3cr3t@127.0.0.1:9?node=127.0.0.1:1",
            budget,
        ))
        .await
        else {
            panic!("unreachable seeds must not connect")
        };
        assert!(
            matches!(cluster, RedisError::Unreachable { .. }),
            "{cluster}"
        );
        let rendered = cluster.to_string();
        assert!(
            rendered.contains("127.0.0.1:9, 127.0.0.1:1") && !rendered.contains("s3cr3t"),
            "{rendered}"
        );

        let Err(sentinel) = RedisConnection::connect(&config(
            "redis-sentinel://alice:s3cr3t@127.0.0.1:9?node=127.0.0.1:1\
             &sentinelServiceName=nestrs&sentinelPassword=s3cr3t",
            budget,
        ))
        .await
        else {
            panic!("unreachable sentinels must not connect")
        };
        assert!(
            matches!(sentinel, RedisError::SentinelUnreachable { .. }),
            "{sentinel}"
        );
        let rendered = sentinel.to_string();
        assert!(
            rendered.contains("127.0.0.1:9, 127.0.0.1:1")
                && rendered.contains("Sentinel")
                && !rendered.contains("s3cr3t"),
            "{rendered}"
        );
    }

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

    #[test]
    fn every_socket_sends_a_command_when_it_is_written() {
        let settings = socket(Duration::from_secs(10));
        assert!(settings.nodelay(), "{settings:?}");
        assert!(settings.keepalive().is_some(), "{settings:?}");
    }

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
