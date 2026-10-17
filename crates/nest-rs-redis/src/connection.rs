//! [`RedisConnection`] — an instance's connection, which every binding over
//! that instance shares to reach Redis, opened once per instance by
//! [`RedisModule`](crate::RedisModule) over the link the topology its URL
//! declares needs: one server
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
use std::time::Duration;

use nest_rs_config::ClientTls;
use nest_rs_core::{Attempt, Backoff, Instance, Proving, Unnamed, Unproved, prove};
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
use crate::instance::RedisInstance;
use crate::script::{Invocation, RedisScript};
use crate::sentinel::SentinelLink;
use crate::standalone::StandaloneLink;
use crate::url::{NodeAddr, RedisUrl};
use crate::{RedisConfig, tls};

/// The sentence every binding's boot error appends when the connection of the
/// instance `I` is missing.
pub(crate) fn connection_remedy<I: Instance>() -> String {
    if I::NAME.is_empty() {
        return "RedisConnection is not registered — import RedisModule::for_root(None), which \
                opens the Redis connection every binding of the default instance shares"
            .to_owned();
    }
    let marker = nest_rs_core::__private::short_type_name::<I>();
    format!(
        "RedisConnection<{marker}> is not registered — import \
         RedisModule::for_root(({marker}, None)), which opens the Redis connection every binding \
         of the instance `{}` shares",
        I::NAME,
    )
}

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
///
/// `I` is the instance it serves: [`Unnamed`], the default, unless the app
/// opened another with `RedisModule::for_root((Cache, cfg))` and injects
/// `Arc<RedisConnection<Cache>>`. Every line it files carries the instance as
/// `instance`, and every refusal names that instance's variables.
pub struct RedisConnection<I = Unnamed> {
    /// The link every clone shares.
    link: Link<I>,
    /// How long a command on this handle waits for its answer.
    budget: Duration,
}

impl<I> Clone for RedisConnection<I> {
    fn clone(&self) -> Self {
        Self {
            link: self.link.clone(),
            budget: self.budget,
        }
    }
}

/// The link a connection reaches Valkey through, one per topology. A
/// Cluster's is generic over the instance: `redis` opens each node's
/// connection through a static constructor, which only a type reaches.
enum Link<I> {
    Standalone(Arc<StandaloneLink>),
    Sentinel(Arc<SentinelLink>),
    Cluster(Arc<ClusterLink<I>>),
    /// A Cluster's link dedicated to one slot's primary.
    Slot(Arc<SlotLink<I>>),
}

impl<I> Clone for Link<I> {
    fn clone(&self) -> Self {
        match self {
            Self::Standalone(link) => Self::Standalone(Arc::clone(link)),
            Self::Sentinel(link) => Self::Sentinel(Arc::clone(link)),
            Self::Cluster(link) => Self::Cluster(Arc::clone(link)),
            Self::Slot(link) => Self::Slot(Arc::clone(link)),
        }
    }
}

/// The shortest the socket waits before it probes, or before it gives up on
/// what it sent: the kernel counts the first in whole seconds and refuses zero.
const LIVENESS_FLOOR: Duration = Duration::from_secs(1);

/// How much each reconnect attempt's wait grows over the last: the boot's
/// doubling.
pub(crate) const RECONNECT_FACTOR: f32 = 2.0;

impl RedisConnection {
    /// Open the default instance's connection to the Redis `config` names and
    /// prove it answers, giving up after
    /// [`connect_timeout`](RedisConfig::connect_timeout).
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
    ///
    /// A named instance's connection is opened by `RedisModule::for_root((Cache,
    /// cfg))`, whose refusals name `<PREFIX>_REDIS__CACHE__*`.
    pub async fn connect(config: &RedisConfig) -> Result<Self, RedisError> {
        Self::open(config).await
    }
}

impl<I: Instance> RedisConnection<I> {
    /// [`connect`](RedisConnection::connect), for the instance `I`: its lines
    /// carry `I`'s name and its refusals name `I`'s variables.
    pub(crate) async fn open(config: &RedisConfig) -> Result<Self, RedisError> {
        let instance = RedisInstance::of::<I>();
        let budget = CONNECT_TIMEOUT
            .check(
                instance.namespace(),
                CONNECT_TIMEOUT.field(),
                config.connect_timeout,
            )
            .map_err(RedisError::Budget)?;
        let link = match RedisUrl::parse(&config.url, &instance)? {
            RedisUrl::Standalone(info) => Link::Standalone(
                StandaloneLink::connect(info, &config.tls, instance, budget).await?,
            ),
            RedisUrl::Sentinel(url) => {
                Link::Sentinel(SentinelLink::connect(url, &config.tls, instance, budget).await?)
            }
            RedisUrl::Cluster(url) => {
                Link::Cluster(ClusterLink::connect(url, &config.tls, instance, budget).await?)
            }
        };
        Ok(Self { link, budget })
    }

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
    /// so a connection seeded without the module is held too. Its setting is
    /// the instance's own variable.
    pub(crate) fn declared_budget() -> nest_rs_core::Budget {
        nest_rs_core::Budget::of::<Self>(
            "the Redis connection",
            format!(
                "{}, or `{}` in code",
                RedisInstance::of::<I>().connect_timeout_var(),
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

    /// The connection [`RedisModule`](crate::RedisModule) opened for the
    /// instance `I`, for `binding`'s factory.
    pub(crate) fn of(container: &nest_rs_core::Container, binding: &str) -> anyhow::Result<Self> {
        container
            .get::<Self>()
            .map(|conn| (*conn).clone())
            .ok_or_else(|| anyhow::anyhow!("{binding}: {}", connection_remedy::<I>()))
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

impl<I: Instance> ConnectionLike for RedisConnection<I> {
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

/// What the proof of `instance`'s link on `endpoint` reports, said on
/// `nest_rs::redis`: every failure that may clear, a budget cut, and a link
/// opened after retrying.
pub(crate) fn said<'a, E: std::error::Error + 'static>(
    instance: &'a RedisInstance,
    endpoint: &'a str,
    budget: Duration,
) -> impl FnMut(Proving<'_, E>) + 'a {
    move |proving| match proving {
        Proving::Retrying { attempt, error } => tracing::warn!(
            target: crate::TARGET,
            instance = instance.name(),
            endpoint = %endpoint,
            attempt,
            error = %nest_rs_core::error_message(error),
            "redis unreachable — retrying within the connect budget",
        ),
        // One final warn, so a hung DNS or a black-holed port is said.
        Proving::TimedOut { attempt } => tracing::warn!(
            target: crate::TARGET,
            instance = instance.name(),
            endpoint = %endpoint,
            attempt,
            timeout_secs = budget.as_secs_f64(),
            "redis connect timed out",
        ),
        Proving::Proved { attempts } if attempts > 1 => tracing::info!(
            target: crate::TARGET,
            instance = instance.name(),
            endpoint = %endpoint,
            attempts,
            "connected to redis after retrying",
        ),
        _ => {}
    }
}

/// The proof of `instance`'s link on `endpoint` within `budget`: [`prove`]
/// over [`Backoff::RECONNECT`]'s curve drawn from `seed` — every replica's
/// own, so replicas that lost Valkey together do not dial it again together —
/// each attempt [`said`].
pub(crate) async fn proved<T, E, F, Fut>(
    instance: &RedisInstance,
    budget: Duration,
    endpoint: &str,
    seed: u64,
    attempt: F,
) -> Result<T, Unproved<E, RedisError>>
where
    F: FnMut(Duration) -> Fut,
    Fut: Future<Output = Result<T, Attempt<E, RedisError>>>,
    E: std::error::Error + 'static,
{
    prove(
        budget,
        Backoff::RECONNECT,
        seed,
        said(instance, endpoint, budget),
        attempt,
    )
    .await
}

/// The boot error of `instance`'s server that answered or failed on every
/// attempt until the budget was spent: not ready when its last failure was an
/// answer, unreachable otherwise.
pub(crate) fn spent(
    instance: &RedisInstance,
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
            connect_timeout_var: instance.connect_timeout_var(),
            source,
        },
        source => RedisError::Unreachable {
            endpoint,
            budget,
            attempts,
            url_var: instance.url_var(),
            connect_timeout_var: instance.connect_timeout_var(),
            source,
        },
    }
}

/// The boot error a proof of `instance`'s `endpoint` that ended without its
/// link ends in.
pub(crate) fn unproved(
    unproved: Unproved<redis::RedisError, RedisError>,
    instance: &RedisInstance,
    endpoint: &str,
    budget: Duration,
) -> RedisError {
    match unproved {
        Unproved::Refused(refusal) => refusal,
        Unproved::Spent { attempts, last } => {
            spent(instance, endpoint.to_owned(), budget, attempts, last)
        }
    }
}

/// A client of one host a topology names — a sentinel, the primary it names,
/// a Cluster's seed — with `settings`, encrypted when `certificates` are
/// given.
pub(crate) fn node_client(
    instance: &RedisInstance,
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
            url_var: instance.url_var(),
            source,
        })?
        .set_redis_settings(settings);
    open_client(instance, info, certificates, &endpoint, budget)
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
    instance: &RedisInstance,
    info: ConnectionInfo,
    certificates: Option<&redis::TlsCertificates>,
    endpoint: &str,
    budget: Duration,
) -> Result<redis::Client, RedisError> {
    let info = info.set_tcp_settings(socket(budget));
    match certificates {
        None => redis::Client::open(info).map_err(|source| RedisError::InvalidUrl {
            endpoint: endpoint.to_owned(),
            url_var: instance.url_var(),
            source,
        }),
        Some(certificates) => {
            redis::Client::build_with_tls(info, certificates.clone()).map_err(|source| {
                RedisError::TlsRefused {
                    endpoint: endpoint.to_owned(),
                    reason: tls::unusable_material(instance),
                    source: Some(source),
                }
            })
        }
    }
}

/// What one failed dial or proof of `instance`'s link met: a refusal every
/// attempt would repeat, or a failure that may clear.
pub(crate) fn classify(
    instance: &RedisInstance,
    source: redis::RedisError,
    endpoint: &str,
    database: i64,
) -> Attempt<redis::RedisError, RedisError> {
    if ClientTls::negotiation_failed(&source) {
        return Attempt::Refused(RedisError::TlsRefused {
            reason: tls::remedy(instance, &source),
            endpoint: endpoint.to_owned(),
            source: Some(source),
        });
    }
    if database_refused(&source) {
        return Attempt::Refused(RedisError::DatabaseRefused {
            endpoint: endpoint.to_owned(),
            database,
            url_var: instance.url_var(),
            source,
        });
    }
    if refused(&source) {
        return Attempt::Refused(RedisError::Refused {
            endpoint: endpoint.to_owned(),
            url_var: instance.url_var(),
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
/// `redis::RedisError::is_timeout` reads both the same way, naming
/// `instance`'s budget.
async fn bounded<F: Future>(
    instance: &RedisInstance,
    budget: Duration,
    call: F,
) -> Result<F::Output, redis::RedisError> {
    #[expect(
        clippy::map_err_ignore,
        reason = "Elapsed carries nothing the timeout's own message does not say"
    )]
    tokio::time::timeout(budget, call).await.map_err(|_| {
        redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "Redis did not answer within {budget:?}, the budget {} sets",
                instance.connect_timeout_var(),
            ),
        ))
    })
}

/// `call`, a command's own outcome on `instance`'s link, answered or failed
/// within `budget`.
pub(crate) async fn answered<T, F>(
    instance: &RedisInstance,
    budget: Duration,
    call: F,
) -> Result<T, redis::RedisError>
where
    F: Future<Output = Result<T, redis::RedisError>>,
{
    bounded(instance, budget, call).await?
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
    use std::time::Instant;

    use super::*;
    use crate::testing::{
        CLIENT, SERVER, answer, config, dropping_then_refusing_listener, inline,
        mutual_tls_listener, presenting, sentinel_listener, silent_listener, tls_listener,
        trusting_the_authority, unnamed,
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
            pinned.tls = trusting_the_authority();
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
            trusting_nothing.tls = ClientTls::new(Some(inline("no certificate in here")), None);
            let mut mismatched = config(url, Duration::from_secs(5));
            mismatched.tls = presenting(&CLIENT.cert, &SERVER.key);

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

    /// TLS 1.3 settles a client certificate after the client's side of the
    /// handshake, so the refusal lands around the first command a link sends —
    /// its `HELLO` — on whichever side of it the race puts it: every topology
    /// hears it as the refusal it is. The tests after this one land it before
    /// that command on every run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_client_certificate_refused_after_the_handshake_fails_the_boot_on_every_topology() {
        let logs = nest_rs_testing::LogCapture::install();
        let (refusing, serving) = mutual_tls_listener().await;
        let (sentinel, naming) = sentinel_listener(refusing).await;
        for (case, url) in [
            ("one server", format!("rediss://{refusing}/")),
            (
                "a sentinel",
                format!("rediss-sentinel://{refusing}?sentinelServiceName=nestrs"),
            ),
            (
                "the primary a sentinel names",
                format!("rediss-sentinel://{sentinel}?sentinelServiceName=nestrs"),
            ),
            ("a Cluster's seed", format!("rediss-cluster://{refusing}")),
        ] {
            let mut pinned = config(&url, Duration::from_secs(5));
            pinned.tls = trusting_the_authority();
            let started = Instant::now();
            let Err(error) = RedisConnection::connect(&pinned).await else {
                panic!("{case}: a client presenting no certificate must be refused")
            };
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "{case}: a refused certificate spends none of the budget, took {:?}",
                started.elapsed(),
            );
            assert!(
                matches!(error, RedisError::TlsRefused { .. }),
                "{case}: {error}"
            );
            let rendered = error.to_string();
            assert!(
                rendered.contains("requires a client certificate")
                    && rendered.contains(&nest_rs_config::var_name("redis", "TLS_CERT")),
                "{case}: the refusal sends the operator to the certificate: {rendered}",
            );
        }
        serving.abort();
        naming.abort();
        logs.expect_none(
            crate::TARGET,
            "redis unreachable — retrying within the connect budget",
        );
    }

    /// The boot of the link `url` names — from the refusing server's address
    /// and that of a sentinel naming it — whose first `HELLO` meets the drop a
    /// refusal landed before it turns into: refused at once, sending the
    /// operator to the certificate, and no attempt retried.
    async fn a_refusal_landed_before_the_first_hello_fails_the_boot(
        url: impl FnOnce(std::net::SocketAddr, std::net::SocketAddr) -> String,
    ) {
        let logs = nest_rs_testing::LogCapture::install();
        let (refusing, serving) = dropping_then_refusing_listener().await;
        let (sentinel, naming) = sentinel_listener(refusing).await;
        let mut pinned = config(&url(refusing, sentinel), Duration::from_secs(5));
        pinned.tls = trusting_the_authority();
        let outcome = RedisConnection::connect(&pinned).await;
        serving.abort();
        naming.abort();

        let Err(error) = outcome else {
            panic!("a client presenting no certificate must be refused")
        };
        assert!(matches!(error, RedisError::TlsRefused { .. }), "{error}");
        let rendered = error.to_string();
        assert!(
            rendered.contains("requires a client certificate")
                && rendered.contains(&nest_rs_config::var_name("redis", "TLS_CERT")),
            "the refusal sends the operator to the certificate: {rendered}",
        );
        logs.expect_none(
            crate::TARGET,
            "redis unreachable — retrying within the connect budget",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_landed_before_one_servers_first_hello_fails_the_boot() {
        a_refusal_landed_before_the_first_hello_fails_the_boot(|refusing, _| {
            format!("rediss://{refusing}/")
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_landed_before_a_sentinels_first_hello_fails_the_boot() {
        a_refusal_landed_before_the_first_hello_fails_the_boot(|refusing, _| {
            format!("rediss-sentinel://{refusing}?sentinelServiceName=nestrs")
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_landed_before_the_named_primarys_first_hello_fails_the_boot() {
        a_refusal_landed_before_the_first_hello_fails_the_boot(|_, sentinel| {
            format!("rediss-sentinel://{sentinel}?sentinelServiceName=nestrs")
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_landed_before_a_cluster_seeds_first_hello_fails_the_boot() {
        a_refusal_landed_before_the_first_hello_fails_the_boot(|refusing, _| {
            format!("rediss-cluster://{refusing}")
        })
        .await;
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

    /// When each attempt of a boot whose every dial is refused is made, under
    /// `seed`, on a paused clock, and how the loop gave up.
    async fn refused_dials(seed: u64, budget: Duration) -> (Vec<Duration>, RedisError) {
        let started = tokio::time::Instant::now();
        let mut made = Vec::new();
        let ended = proved(&unnamed(), budget, "127.0.0.1:9", seed, |_| {
            made.push(started.elapsed());
            async {
                Err::<(), _>(Attempt::Failed(redis::RedisError::from(
                    std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
                )))
            }
        })
        .await
        .expect_err("a dial refused every time never opens");
        (made, unproved(ended, &unnamed(), "127.0.0.1:9", budget))
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_climb_jittered_apart_for_two_seeds_then_fail_unreachable() {
        let budget = Duration::from_secs(10);
        let (one, refused) = refused_dials(1, budget).await;
        let (other, _) = refused_dials(2, budget).await;

        assert_ne!(one, other, "two replicas' loops come back apart");
        for made in [&one, &other] {
            let waits: Vec<Duration> = made.windows(2).map(|pair| pair[1] - pair[0]).collect();
            for (failed, wait) in waits.iter().take(3).enumerate() {
                let curve = Backoff::RECONNECT.first() * (1 << failed);
                assert!(
                    *wait >= curve.mul_f64(0.8) && *wait <= curve.mul_f64(1.2),
                    "wait {failed}: {wait:?} against {curve:?} in {made:?}",
                );
            }
            assert!(
                waits
                    .iter()
                    .all(|wait| *wait <= Backoff::RECONNECT.ceiling().mul_f64(1.2)),
                "{waits:?}"
            );
            assert!(made.last().is_some_and(|last| *last < budget), "{made:?}");
        }
        assert!(
            matches!(refused, RedisError::Unreachable { .. }),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn a_refused_endpoint_announces_every_attempt_and_fails_naming_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let started = Instant::now();
        // Port 9 is `discard` — reserved and never listening, so every attempt
        // is refused at once. The budget sits above `Backoff::RECONNECT`'s first wait (250ms)
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

    #[nest_rs_core::instance]
    struct Cache;

    /// What `open` of the instance `Cache` refuses `config` with.
    async fn refused_for_the_cache(config: RedisConfig) -> RedisError {
        match RedisConnection::<Cache>::open(&config).await {
            Ok(_) => panic!("{config:?} must not connect"),
            Err(refused) => refused,
        }
    }

    /// Every refusal names the variables of the instance that failed, never the
    /// default's.
    #[tokio::test]
    async fn a_named_instance_s_refusals_name_its_own_variables() {
        let cache = |key| nest_rs_config::var_name("redis__cache", key);
        let default = |key| nest_rs_config::var_name("redis", key);
        let mut plaintext = config("redis://127.0.0.1:9/", Duration::from_secs(5));
        plaintext.tls = trusting_the_authority();
        for (case, refused, named) in [
            (
                "a zero budget",
                refused_for_the_cache(config("redis://127.0.0.1:9/", Duration::ZERO)).await,
                vec![cache("CONNECT_TIMEOUT_SECS")],
            ),
            (
                "RESP3",
                refused_for_the_cache(config(
                    "redis://127.0.0.1:9/?protocol=resp3",
                    Duration::from_secs(5),
                ))
                .await,
                vec![cache("URL")],
            ),
            (
                "a skipped verification",
                refused_for_the_cache(config(
                    "rediss://127.0.0.1:9/#insecure",
                    Duration::from_secs(5),
                ))
                .await,
                vec![cache("URL"), cache("TLS_CA_CERT")],
            ),
            (
                "material beside a plaintext URL",
                refused_for_the_cache(plaintext).await,
                vec![cache("URL"), cache("TLS_CA_CERT"), cache("TLS_KEY")],
            ),
        ] {
            let rendered = refused.to_string();
            for variable in named {
                assert!(rendered.contains(&variable), "{case}: {rendered}");
            }
            for variable in ["URL", "CONNECT_TIMEOUT_SECS", "TLS_CA_CERT", "TLS_KEY"] {
                assert!(!rendered.contains(&default(variable)), "{case}: {rendered}");
            }
        }
    }

    #[tokio::test]
    async fn a_named_instance_s_unreachable_server_is_said_and_refused_as_that_instance() {
        let logs = nest_rs_testing::LogCapture::install();
        let refused =
            refused_for_the_cache(config("redis://127.0.0.1:9/", Duration::from_millis(700))).await;
        assert!(
            matches!(refused, RedisError::Unreachable { .. }),
            "{refused}"
        );
        let rendered = refused.to_string();
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis__cache", "URL"))
                && rendered.contains(&nest_rs_config::var_name(
                    "redis__cache",
                    "CONNECT_TIMEOUT_SECS"
                )),
            "{rendered}"
        );
        let retries = logs.find(
            crate::TARGET,
            "redis unreachable — retrying within the connect budget",
        );
        assert!(!retries.is_empty(), "{:#?}", logs.events());
        for event in retries {
            assert_eq!(event.field("instance").as_deref(), Some("cache"));
        }
    }

    #[test]
    fn a_missing_connection_sends_the_binding_to_its_instance_s_import() {
        assert!(
            connection_remedy::<Unnamed>().contains("import RedisModule::for_root(None)"),
            "{}",
            connection_remedy::<Unnamed>()
        );
        let named = connection_remedy::<Cache>();
        assert!(
            named.starts_with("RedisConnection<Cache> is not registered")
                && named.contains("import RedisModule::for_root((Cache, None))")
                && named.contains("the instance `cache`"),
            "{named}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_named_instance_s_command_timeout_names_its_budget() {
        let error = bounded(
            &crate::instance::RedisInstance::of::<Cache>(),
            Duration::from_millis(50),
            std::future::pending::<()>(),
        )
        .await
        .expect_err("a call that never answers is ended by the budget");
        assert!(
            error.to_string().contains(&nest_rs_config::var_name(
                "redis__cache",
                "CONNECT_TIMEOUT_SECS"
            )),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_command_that_never_answers_fails_at_the_budget_as_a_timeout() {
        let budget = Duration::from_millis(150);
        let started = Instant::now();
        let error = bounded(&unnamed(), budget, std::future::pending::<()>())
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
