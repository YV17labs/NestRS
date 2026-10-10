//! [`StandaloneLink`] — the link to one server: one `redis`
//! [`ConnectionManager`], one multiplexed socket reopened behind its callers
//! when Valkey drops it.
//!
//! **A connection answered `READONLY` is opened again**: a failover behind a
//! name the DNS follows demotes the primary under an open socket, and `redis`
//! reopens only a socket that fails.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{Cmd, ConnectionAddr, ConnectionInfo, Pipeline, Value};

use crate::connection::{
    Attempt, FIRST_RETRY_BACKOFF, MAX_RETRY_BACKOFF, RECONNECT_FACTOR, Replies, answered,
    backoff_after, classify, demoted, dial, open_client, refused, within_budget,
};
use crate::error::RedisError;
use crate::topology::Hello;
use crate::{RedisTls, RedisTopology, tls};

/// The link to one server, and what opening it again needs.
pub(crate) struct StandaloneLink {
    /// The manager, and how many times it was opened again: a read-only answer
    /// reopens only the manager that gave it, never its successor.
    manager: RwLock<(u64, ConnectionManager)>,
    /// The client the link was opened from, which a reopened or dedicated one
    /// is opened from too: same address, same TLS material and verification.
    client: redis::Client,
    endpoint: String,
    /// The connect budget, which bounds opening the link again.
    budget: Duration,
    /// TLS refusals met reopening the link — `None` over plaintext, where no
    /// handshake can be refused.
    refusals: Option<Arc<TlsRefusals>>,
    reopening: AtomicBool,
}

impl StandaloneLink {
    /// Open the link to the server `info` names, proved within `budget`.
    pub(crate) async fn connect(
        info: ConnectionInfo,
        tls: &RedisTls,
        budget: Duration,
    ) -> Result<Arc<Self>, RedisError> {
        let endpoint = info.addr().to_string();
        let client = client(info, tls, &endpoint, budget)?;
        let database = client.get_connection_info().redis_settings().db();
        let manager = within_budget(budget, &endpoint, |_| {
            prove(&client, budget, &endpoint, database)
        })
        .await
        .map_err(|gave_up| gave_up.into_error(&endpoint, budget))?;
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
        Ok(Self::new(manager, client, endpoint, budget, refusals))
    }

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

    /// A link of its own to the same server, opened from the same client.
    pub(crate) async fn dedicated(&self) -> Result<Arc<Self>, redis::RedisError> {
        let manager = self.open_manager().await?;
        Ok(Self::new(
            manager,
            self.client.clone(),
            self.endpoint.clone(),
            self.budget,
            self.refusals.clone(),
        ))
    }

    /// Send `cmd`, answered or failed within `budget`.
    pub(crate) async fn send(
        self: &Arc<Self>,
        cmd: &Cmd,
        budget: Duration,
    ) -> Result<Value, redis::RedisError> {
        let (opening, mut manager) = self.manager();
        let outcome = answered(budget, manager.send_packed_command(cmd)).await;
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
        let (opening, mut manager) = self.manager();
        let outcome = answered(
            budget,
            manager.send_packed_commands(pipeline, offset, count),
        )
        .await;
        self.observe(opening, &outcome);
        outcome
    }

    /// The database the URL selects.
    pub(crate) fn db(&self) -> i64 {
        self.client.get_connection_info().redis_settings().db()
    }

    /// A manager of its own from the link's client, opened within the budget.
    async fn open_manager(&self) -> Result<ConnectionManager, redis::RedisError> {
        answered(
            self.budget,
            ConnectionManager::new_with_config(self.client.clone(), manager_config(self.budget)),
        )
        .await
    }

    /// The manager commands go through now, and its opening.
    fn manager(&self) -> (u64, ConnectionManager) {
        self.manager
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Read what a command through the manager of `opening` met.
    fn observe<T: Replies>(self: &Arc<Self>, opening: u64, outcome: &Result<T, redis::RedisError>) {
        if let Some(refusals) = &self.refusals {
            refusals.observe(outcome);
        }
        if demoted(outcome) {
            self.reopen(opening);
        }
    }

    /// Open the link afresh, unless the manager of `opening` was replaced
    /// already or an opening is under way.
    fn reopen(self: &Arc<Self>, opening: u64) {
        if self.manager().0 != opening || self.reopening.swap(true, Ordering::SeqCst) {
            return;
        }
        let link = Arc::clone(self);
        tokio::spawn(async move {
            match link.open_manager().await {
                Ok(manager) => {
                    let mut kept = link.manager.write().unwrap_or_else(PoisonError::into_inner);
                    if kept.0 == opening {
                        *kept = (opening + 1, manager);
                        drop(kept);
                        tracing::warn!(
                            target: crate::TARGET,
                            endpoint = %link.endpoint,
                            "redis connection opened again: the server it reached answered as a \
                             read-only replica",
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    target: crate::TARGET,
                    endpoint = %link.endpoint,
                    error = %nest_rs_core::error_message(&error),
                    "redis connection answered as a read-only replica and not opened again; \
                     the next read-only answer tries again",
                ),
            }
            link.reopening.store(false, Ordering::SeqCst);
        });
    }
}

/// Whether the link has been refused by TLS since it last answered, and how
/// to learn why.
///
/// `redis` reopens silently and copies an io error by its message, so rustls's
/// reason never reaches a caller: the first command failing on the socket starts
/// one handshake of the link's own, whose refusal is reported at `warn`.
struct TlsRefusals {
    /// The client the link is opened from, so the diagnosing handshake is the
    /// one the client keeps failing: same address, same material.
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
            tls::say_refused(&self.endpoint, error);
        }
    }

    /// One handshake per budget at most, and none while the refusal stands
    /// reported: a Redis that is down fails every command on the socket. It
    /// carries no unit of work's trace: the refusal is the link's.
    fn diagnose(self: &Arc<Self>) {
        if self.reported.load(Ordering::Relaxed) || self.diagnosing.swap(true, Ordering::Relaxed) {
            return;
        }
        let refusals = Arc::clone(self);
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            if let Some(refused) = tls::refusal(&refusals.client, refusals.budget).await {
                refusals.report(&refused);
            }
            tokio::time::sleep_until(started + refusals.budget).await;
            refusals.diagnosing.store(false, Ordering::Relaxed);
        });
    }
}

/// The client the link is opened from, and reopened from, so the URL's TLS and
/// the material beside it reach every connection it opens. Nothing is dialled:
/// each refusal here is one every attempt would repeat.
fn client(
    info: ConnectionInfo,
    tls: &RedisTls,
    endpoint: &str,
    budget: Duration,
) -> Result<redis::Client, RedisError> {
    let encrypted = matches!(info.addr(), ConnectionAddr::TcpTls { .. });
    let certificates = tls::material(tls, encrypted, endpoint)?;
    open_client(info, certificates.as_ref(), endpoint, budget)
}

/// One boot attempt: a connection of its own, proved a server of its own by
/// its `HELLO` and with a `PING`, then the connection the app keeps.
///
/// The proof runs on a connection opened once, with no retry: the kept one's
/// client retries silently, refused credentials included. The proof is closed
/// before the kept connection opens, so an attempt needs one of the server's
/// `maxclients` slots, not two.
async fn prove(
    client: &redis::Client,
    budget: Duration,
    endpoint: &str,
    database: i64,
) -> Result<ConnectionManager, Attempt<redis::RedisError>> {
    let classified = |source| classify(source, endpoint, database);
    let mut proof = dial(client, budget).await.map_err(classified)?;
    let hello = match Hello::ask(&mut proof).await {
        Ok(hello) => hello,
        Err(dropped) if dropped.is_connection_dropped() => {
            return Err(classified(
                tls::refusal(client, budget).await.unwrap_or(dropped),
            ));
        }
        Err(error) => return Err(classified(error)),
    };
    if hello.serves != RedisTopology::Standalone {
        return Err(Attempt::Refused(RedisError::TopologyMismatch {
            endpoint: endpoint.to_owned(),
            declared: RedisTopology::Standalone,
            serves: hello.serves,
        }));
    }
    redis::cmd("PING")
        .query_async::<()>(&mut proof)
        .await
        .map_err(classified)?;
    drop(proof);
    // The kept connection is proved too: one sending nothing on opening meets a
    // full `maxclients` only at its first command. Same manager: one dropped
    // mid-reopen would still take a slot.
    let mut kept = ConnectionManager::new_with_config(client.clone(), manager_config(budget))
        .await
        .map_err(classified)?;
    let mut wait = FIRST_RETRY_BACKOFF;
    loop {
        match redis::cmd("PING").query_async::<()>(&mut kept).await {
            Ok(()) => return Ok(kept),
            Err(error) if refused(&error) || tls::negotiation_failed(&error) => {
                return Err(classified(error));
            }
            Err(_) => {
                tokio::time::sleep(wait).await;
                wait = backoff_after(wait);
            }
        }
    }
}

/// How the kept connection reopens after Redis drops it: each attempt bounded
/// by the budget, the backoff doubling to the boot's ceiling. The reply timeout
/// stays off: [`RedisConnection`](crate::RedisConnection) bounds every command
/// itself.
fn manager_config(budget: Duration) -> ConnectionManagerConfig {
    ConnectionManagerConfig::new()
        .set_connection_timeout(Some(budget))
        .set_response_timeout(None)
        .set_min_delay(FIRST_RETRY_BACKOFF)
        .set_exponent_base(RECONNECT_FACTOR)
        .set_max_delay(MAX_RETRY_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AUTHORITY, answer, config, mutual_tls_listener, tls_listener};
    use crate::url::{RedisUrl, address};

    use std::time::Instant;

    const REFUSED: &str = "redis refused a reopened tls connection";

    /// A refusal's record, over a client opened from `url` as the boot opens
    /// it — so rustls has its process-default provider when the record
    /// handshakes.
    fn refusals(url: &str) -> Arc<TlsRefusals> {
        refusals_within(url, Duration::from_secs(2))
    }

    /// [`refusals`] under `budget`.
    fn refusals_within(url: &str, budget: Duration) -> Arc<TlsRefusals> {
        refusals_with(url, budget, &config(url, budget).tls)
    }

    /// [`refusals_within`], with `tls`'s material.
    fn refusals_with(url: &str, budget: Duration, tls: &RedisTls) -> Arc<TlsRefusals> {
        let endpoint = address(url);
        let Ok(RedisUrl::Standalone(info)) = RedisUrl::parse(url) else {
            panic!("{url} is a URL of one server");
        };
        Arc::new(TlsRefusals {
            client: client(info, tls, &endpoint, budget).expect("the URL opens a client"),
            endpoint,
            budget,
            reported: AtomicBool::new(false),
            diagnosing: AtomicBool::new(false),
        })
    }

    /// The test authority trusted, and no certificate presented.
    fn presenting_none() -> RedisTls {
        RedisTls {
            ca_cert: Some(AUTHORITY.pem().as_bytes().to_vec()),
            identity: None,
        }
    }

    /// Waits up to five seconds for `logs` to hold the reopen's refusal.
    async fn reported(logs: &nest_rs_testing::LogCapture) -> Vec<nest_rs_testing::CapturedEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while logs.find(crate::TARGET, REFUSED).is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        logs.find(crate::TARGET, REFUSED)
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

    #[tokio::test]
    async fn a_refusal_known_only_by_its_text_is_learnt_from_a_handshake_of_its_own() {
        let logs = nest_rs_testing::LogCapture::install();
        let (addr, serving) = tls_listener().await;
        let refusals = refusals(&format!("rediss://{addr}/"));

        refusals.observe(&reopening_refused());
        refusals.observe(&reopening_refused());
        let reported = reported(&logs).await;
        serving.abort();

        assert_eq!(reported.len(), 1, "{:#?}", logs.events());
        let reason = reported[0].field("reason").unwrap_or_default();
        assert!(
            reason.contains("does not chain to an authority")
                && reason.contains(&nest_rs_config::var_name("redis", "TLS_CA_CERT")),
            "the handshake's own reason, sending the operator to the authority: {reason}",
        );
    }

    /// Why the boot asks: a refusal that lands before a connection's first
    /// command is in flight reaches that command as a dropped connection, and
    /// only a handshake of its own still hears it.
    #[tokio::test]
    async fn a_refusal_landing_before_the_first_command_reaches_it_as_a_dropped_connection() {
        let (addr, serving) = mutual_tls_listener().await;
        let url = format!("rediss://{addr}/");
        let Ok(RedisUrl::Standalone(info)) = RedisUrl::parse(&url) else {
            panic!("{url} is a URL of one server");
        };
        let budget = Duration::from_secs(2);
        let client =
            client(info, &presenting_none(), &address(&url), budget).expect("a client opens");

        let mut connection = dial(&client, budget)
            .await
            .expect("the client's side of the handshake completes");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Err(met) = Hello::ask(&mut connection).await else {
            panic!("a Redis refusing the client's certificate answers nothing");
        };
        let refused = tls::refusal(&client, budget).await;
        serving.abort();

        assert!(
            met.is_connection_dropped() && !tls::negotiation_failed(&met),
            "the first command meets a dropped connection, not the refusal: {met}",
        );
        let Some(refused) = refused else {
            panic!("a handshake of its own hears the refusal");
        };
        assert!(
            tls::remedy(&refused).contains("requires a client certificate"),
            "{refused}"
        );
    }

    /// TLS 1.3 refuses a client certificate after the client's side of the
    /// handshake is over, so a handshake alone never meets that refusal.
    #[tokio::test]
    async fn a_client_certificate_refused_after_the_handshake_is_learnt_all_the_same() {
        let logs = nest_rs_testing::LogCapture::install();
        let (addr, serving) = mutual_tls_listener().await;
        let refusals = refusals_with(
            &format!("rediss://{addr}/"),
            Duration::from_secs(2),
            &presenting_none(),
        );

        refusals.observe(&connection_dropped());
        let reported = reported(&logs).await;
        serving.abort();

        assert_eq!(reported.len(), 1, "{:#?}", logs.events());
        let reason = reported[0].field("reason").unwrap_or_default();
        assert!(
            reason.contains("requires a client certificate")
                && reason.contains(&nest_rs_config::var_name("redis", "TLS_CERT")),
            "the refusal's own reason, sending the operator to the certificate: {reason}",
        );
    }

    /// The boot's first command meets the same refusal on a connection whose
    /// opening read nothing: it fails the boot on that attempt, retried by none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_client_certificate_refused_after_the_handshake_fails_the_boot_unretried() {
        let logs = nest_rs_testing::LogCapture::install();
        let (addr, serving) = mutual_tls_listener().await;
        let url = format!("rediss://{addr}/");
        let Ok(RedisUrl::Standalone(info)) = RedisUrl::parse(&url) else {
            panic!("{url} is a URL of one server");
        };

        let outcome =
            StandaloneLink::connect(info, &presenting_none(), Duration::from_secs(5)).await;
        serving.abort();

        let Err(error) = outcome else {
            panic!("a Redis requiring a client certificate must refuse a client presenting none")
        };
        assert!(matches!(error, RedisError::TlsRefused { .. }), "{error}");
        assert!(
            error.to_string().contains("requires a client certificate"),
            "{error}"
        );
        logs.expect_none(
            crate::TARGET,
            "redis unreachable — retrying within the connect budget",
        );
    }

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
}
