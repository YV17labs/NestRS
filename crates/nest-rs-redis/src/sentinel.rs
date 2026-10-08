//! [`SentinelLink`] — the link to the primary the sentinels name, found as
//! Valkey's Sentinel client spec has a client find it: each sentinel is asked in
//! turn for the address of the primary named `sentinelServiceName`
//! (`SENTINEL GET-MASTER-ADDR-BY-NAME`), the one that answers is asked first
//! next time, and the address it names is kept only once it says it is a
//! primary. The spec asks `ROLE`; `HELLO` answers the same and needs no
//! permission, so a user's rule grants one command fewer.
//!
//! **Every reconnection asks the sentinels again**, as the spec requires: a
//! connection that drops, times out or answers `READONLY` is replaced by one to
//! the primary the sentinels name then, never reopened to the address it had —
//! after a failover, that address is a replica. A command sent meanwhile waits
//! for the new connection, within its budget.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use redis::aio::MultiplexedConnection;
use redis::{Cmd, ErrorKind, Pipeline, RedisConnectionInfo, Value};
use tokio::sync::Notify;

use crate::connection::{
    Attempt, FIRST_RETRY_BACKOFF, GaveUp, Replies, answered, backoff_after, classify, demoted,
    dial, node_client, spent, within_budget,
};
use crate::error::{RedisError, SentinelMiss};
use crate::topology::Hello;
use crate::url::{NodeAddr, SentinelUrl, listed};
use crate::{RedisTls, RedisTopology, tls};

/// The link to the primary the sentinels name.
pub(crate) struct SentinelLink {
    sentinels: Arc<Sentinels>,
    current: RwLock<Current>,
    /// Woken when a replacement connection is in place.
    ready: Notify,
}

/// The connection commands go through now.
struct Current {
    /// How many times the link was opened again: a failure replaces only the
    /// connection that met it, never its successor.
    opening: u64,
    /// The connection to the primary, `None` while the sentinels are asked again.
    connection: Option<MultiplexedConnection>,
    /// The primary's address.
    primary: String,
}

/// What finding the primary needs, shared by a link and every link it
/// dedicates.
struct Sentinels {
    /// Each sentinel's client, and its address.
    sentinels: Vec<(redis::Client, String)>,
    /// Every sentinel's address, as an error names them.
    listed: String,
    service_name: String,
    /// The primary's credentials and database.
    primary_settings: RedisConnectionInfo,
    certificates: Option<redis::TlsCertificates>,
    budget: Duration,
    /// The sentinel that answered last, asked first.
    first: AtomicUsize,
    /// Whether a failure finding the primary again is said already.
    said: AtomicBool,
}

impl SentinelLink {
    /// Open the link to the primary the sentinels `url` lists name, proved
    /// within `budget`.
    pub(crate) async fn connect(
        url: SentinelUrl,
        tls: &RedisTls,
        budget: Duration,
    ) -> Result<Arc<Self>, RedisError> {
        let listed = listed(&url.sentinels);
        let certificates = tls::material(tls, url.tls, &listed)?;
        let sentinels = url
            .sentinels
            .iter()
            .map(|sentinel| {
                let endpoint = sentinel.to_string();
                let client = node_client(
                    sentinel,
                    url.sentinel_settings.clone(),
                    certificates.as_ref(),
                    budget,
                )?;
                Ok((client, endpoint))
            })
            .collect::<Result<Vec<_>, RedisError>>()?;
        let sentinels = Arc::new(Sentinels {
            sentinels,
            listed,
            service_name: url.service_name,
            primary_settings: url.primary_settings,
            certificates,
            budget,
            first: AtomicUsize::new(0),
            said: AtomicBool::new(false),
        });
        let (connection, primary) = within_budget(budget, &sentinels.listed, |remaining| {
            sentinels.resolve(remaining)
        })
        .await
        .map_err(|gave_up| sentinels.gave_up(gave_up))?;
        Ok(Self::opened(sentinels, connection, primary))
    }

    fn opened(
        sentinels: Arc<Sentinels>,
        connection: MultiplexedConnection,
        primary: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            sentinels,
            current: RwLock::new(Current {
                opening: 0,
                connection: Some(connection),
                primary,
            }),
            ready: Notify::new(),
        })
    }

    /// A link of its own to the primary the sentinels name now.
    pub(crate) async fn dedicated(&self) -> Result<Arc<Self>, redis::RedisError> {
        let sentinels = Arc::clone(&self.sentinels);
        match sentinels.resolve(sentinels.budget).await {
            Ok((connection, primary)) => Ok(Self::opened(sentinels, connection, primary)),
            Err(Attempt::Refused(refusal)) => {
                Err(unreachable(nest_rs_core::error_message(&refusal)))
            }
            Err(Attempt::Failed(miss)) => Err(miss.into()),
        }
    }

    /// Send `cmd`, answered or failed within `budget`, the wait for a
    /// replacement connection included.
    pub(crate) async fn send(
        self: &Arc<Self>,
        cmd: &Cmd,
        budget: Duration,
    ) -> Result<Value, redis::RedisError> {
        let mut opening = None;
        let outcome = answered(budget, async {
            let (at, mut connection) = self.connection().await;
            opening = Some(at);
            connection.send_packed_command(cmd).await
        })
        .await;
        self.observe(opening, &outcome);
        outcome
    }

    /// Send `count` replies' worth of `pipeline` from `offset`, answered or
    /// failed within `budget`.
    pub(crate) async fn send_pipeline(
        self: &Arc<Self>,
        pipeline: &Pipeline,
        offset: usize,
        count: usize,
        budget: Duration,
    ) -> Result<Vec<Value>, redis::RedisError> {
        let mut opening = None;
        let outcome = answered(budget, async {
            let (at, mut connection) = self.connection().await;
            opening = Some(at);
            connection
                .send_packed_commands(pipeline, offset, count)
                .await
        })
        .await;
        self.observe(opening, &outcome);
        outcome
    }

    /// The database the URL selects on the primary.
    pub(crate) fn db(&self) -> i64 {
        self.sentinels.primary_settings.db()
    }

    /// The connection commands go through now, and its opening — waiting for
    /// a replacement while the sentinels are asked again.
    async fn connection(&self) -> (u64, MultiplexedConnection) {
        loop {
            if let Some(open) = self.open() {
                return open;
            }
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(open) = self.open() {
                return open;
            }
            ready.await;
        }
    }

    /// The connection in place, and its opening, if one is.
    fn open(&self) -> Option<(u64, MultiplexedConnection)> {
        let current = self.current.read().unwrap_or_else(PoisonError::into_inner);
        current
            .connection
            .clone()
            .map(|connection| (current.opening, connection))
    }

    /// Replace the connection of `opening` when what it met says it no longer
    /// reaches the primary: a drop, a timeout, a read-only answer.
    fn observe<T: Replies>(
        self: &Arc<Self>,
        opening: Option<u64>,
        outcome: &Result<T, redis::RedisError>,
    ) {
        let Some(opening) = opening else {
            return;
        };
        let dropped = outcome.as_ref().is_err_and(|error| {
            error.is_timeout() || error.is_unrecoverable_error() || error.is_connection_dropped()
        });
        if dropped || demoted(outcome) {
            self.lost(opening);
        }
    }

    /// Drop the connection of `opening`, and ask the sentinels again for the
    /// primary until one opens.
    fn lost(self: &Arc<Self>, opening: u64) {
        {
            let mut current = self.current.write().unwrap_or_else(PoisonError::into_inner);
            if current.opening != opening || current.connection.is_none() {
                return;
            }
            // Only the call that empties it reopens: one reopening at a time.
            current.connection = None;
        }
        tokio::spawn(Self::reopen(Arc::downgrade(self)));
    }

    async fn reopen(link: Weak<Self>) {
        let mut wait = FIRST_RETRY_BACKOFF;
        loop {
            let Some(this) = link.upgrade() else {
                return;
            };
            match this.sentinels.resolve(this.sentinels.budget).await {
                Ok((connection, primary)) => {
                    this.sentinels.said.store(false, Ordering::Relaxed);
                    let previous = {
                        let mut current =
                            this.current.write().unwrap_or_else(PoisonError::into_inner);
                        current.opening += 1;
                        current.connection = Some(connection);
                        std::mem::replace(&mut current.primary, primary.clone())
                    };
                    this.ready.notify_waiters();
                    if previous == primary {
                        tracing::info!(
                            target: crate::TARGET,
                            primary = %primary,
                            "redis connection to the primary the sentinels name opened again",
                        );
                    } else {
                        tracing::warn!(
                            target: crate::TARGET,
                            primary = %primary,
                            previous = %previous,
                            "redis connection follows the new primary the sentinels name",
                        );
                    }
                    return;
                }
                Err(failure) => this.sentinels.say(&failure),
            }
            drop(this);
            tokio::time::sleep(wait).await;
            wait = backoff_after(wait);
        }
    }
}

impl Sentinels {
    /// One round: each sentinel asked in turn, from the one that answered
    /// last, for the primary's address, and the primary it names proved with
    /// `HELLO` — within `within`.
    async fn resolve(
        &self,
        within: Duration,
    ) -> Result<(MultiplexedConnection, String), Attempt<SentinelMiss>> {
        let deadline = Instant::now() + within;
        let count = self.sentinels.len();
        // Each sentinel gets an equal share of the round, so one that never
        // answers leaves the others theirs.
        let share = within / u32::try_from(count).unwrap_or(u32::MAX).max(1);
        let first = self.first.load(Ordering::Relaxed);
        let mut unknown = false;
        let mut last = None;
        for turn in 0..count {
            let at = (first + turn) % count;
            let (client, endpoint) = &self.sentinels[at];
            let remaining = deadline.saturating_duration_since(Instant::now());
            match answered(share.min(remaining), self.ask(client, endpoint)).await {
                Ok(Answer::Primary(primary)) => {
                    self.first.store(at, Ordering::Relaxed);
                    return self.open_primary(&primary, deadline).await;
                }
                Ok(Answer::Unknown) => unknown = true,
                Ok(Answer::Serves(serves)) => {
                    return Err(Attempt::Refused(RedisError::TopologyMismatch {
                        endpoint: endpoint.clone(),
                        declared: RedisTopology::Sentinel,
                        serves,
                    }));
                }
                Err(error) => match classify(error, endpoint, 0) {
                    Attempt::Refused(refusal) => return Err(Attempt::Refused(refusal)),
                    Attempt::Failed(error) => last = Some(error),
                },
            }
        }
        Err(Attempt::Failed(if unknown {
            SentinelMiss::Unknown
        } else {
            SentinelMiss::Unreachable(last)
        }))
    }

    /// What one sentinel answers when asked for the primary's address.
    async fn ask(
        &self,
        client: &redis::Client,
        endpoint: &str,
    ) -> Result<Answer, redis::RedisError> {
        let opened = dial(client, self.budget).await;
        tls::observe_refusal(endpoint, &opened);
        let mut sentinel = opened?;
        let hello = Hello::ask(&mut sentinel).await?;
        if hello.serves != RedisTopology::Sentinel {
            return Ok(Answer::Serves(hello.serves));
        }
        let named: Option<(String, u16)> = redis::cmd("SENTINEL")
            .arg("GET-MASTER-ADDR-BY-NAME")
            .arg(&self.service_name)
            .query_async(&mut sentinel)
            .await?;
        Ok(named.map_or(Answer::Unknown, |(host, port)| {
            Answer::Primary(NodeAddr { host, port })
        }))
    }

    /// A connection to the address the sentinels named, kept once `HELLO` says
    /// it is a primary and it answers a `PING`.
    async fn open_primary(
        &self,
        primary: &NodeAddr,
        deadline: Instant,
    ) -> Result<(MultiplexedConnection, String), Attempt<SentinelMiss>> {
        let endpoint = primary.to_string();
        let database = self.primary_settings.db();
        let missed = |source| {
            Attempt::Failed(SentinelMiss::Primary {
                endpoint: endpoint.clone(),
                source,
            })
        };
        let failed = |error| match classify(error, &endpoint, database) {
            Attempt::Refused(refusal) => Attempt::Refused(refusal),
            Attempt::Failed(error) => missed(Some(error)),
        };
        let client = node_client(
            primary,
            self.primary_settings.clone(),
            self.certificates.as_ref(),
            self.budget,
        )
        .map_err(Attempt::Refused)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let opened = answered(remaining, dial(&client, self.budget)).await;
        tls::observe_refusal(&endpoint, &opened);
        let mut connection = opened.map_err(failed)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let hello = answered(remaining, Hello::ask(&mut connection))
            .await
            .map_err(failed)?;
        if !hello.primary {
            // The sentinels still name a node a failover demoted, or one that
            // is no primary yet.
            return Err(missed(None));
        }
        // The proof every topology's connection passes: one rule per role
        // holds on all of them.
        let remaining = deadline.saturating_duration_since(Instant::now());
        answered(
            remaining,
            redis::cmd("PING").query_async::<()>(&mut connection),
        )
        .await
        .map_err(failed)?;
        Ok((connection, endpoint))
    }

    /// The boot error a spent or refused round of attempts ends in.
    fn gave_up(&self, gave_up: GaveUp<SentinelMiss>) -> RedisError {
        let (attempts, last) = match gave_up {
            GaveUp::Refused(refusal) => return refusal,
            GaveUp::Spent { attempts, last } => (attempts, last),
        };
        match last.unwrap_or(SentinelMiss::Unreachable(None)) {
            SentinelMiss::Unknown => RedisError::PrimaryUnknown {
                sentinels: self.listed.clone(),
                service_name: self.service_name.clone(),
                budget: self.budget,
                attempts,
            },
            SentinelMiss::Primary { endpoint, source } => {
                spent(endpoint, self.budget, attempts, source)
            }
            SentinelMiss::Unreachable(source) => RedisError::SentinelUnreachable {
                sentinels: self.listed.clone(),
                budget: self.budget,
                attempts,
                source,
            },
        }
    }

    /// Say a failure to find the primary again: once at `warn` until a
    /// connection opens, then at `debug`, so an outage writes one line.
    fn say(&self, failure: &Attempt<SentinelMiss>) {
        let first = !self.said.swap(true, Ordering::Relaxed);
        let error = match failure {
            Attempt::Refused(refusal) => nest_rs_core::error_message(refusal),
            Attempt::Failed(miss) => nest_rs_core::error_message(miss),
        };
        if first {
            tracing::warn!(
                target: crate::TARGET,
                sentinels = %self.listed,
                error = %error,
                "redis primary not found again through the sentinels yet; retrying",
            );
        } else {
            tracing::debug!(
                target: crate::TARGET,
                sentinels = %self.listed,
                error = %error,
                "redis primary still not found again through the sentinels",
            );
        }
    }
}

impl From<SentinelMiss> for redis::RedisError {
    fn from(miss: SentinelMiss) -> Self {
        match miss {
            SentinelMiss::Unreachable(Some(source))
            | SentinelMiss::Primary {
                source: Some(source),
                ..
            } => source,
            miss => unreachable(miss.to_string()),
        }
    }
}

/// What one sentinel answered when asked for the primary's address.
enum Answer {
    /// The primary's address.
    Primary(NodeAddr),
    /// It knows no primary by the service's name.
    Unknown,
    /// It is no sentinel: it serves this topology.
    Serves(RedisTopology),
}

/// What a dedicated link that found no primary hands its caller.
fn unreachable(detail: String) -> redis::RedisError {
    redis::RedisError::from((
        ErrorKind::Io,
        "the primary the sentinels name could not be reached",
        detail,
    ))
}
