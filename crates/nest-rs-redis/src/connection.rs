//! [`RedisConnection`] — the one connection every binding in this crate shares
//! to reach Redis. Opened once by [`RedisModule`](crate::RedisModule) in the
//! collect phase; the queue producer, the worker and the rate-limit store each
//! read it from the container rather than opening a socket of their own.
//!
//! **It is a connection, not a factory of them.** It implements
//! [`ConnectionLike`], so apalis takes it as the connection its storage runs on,
//! the rate limiter runs its script on a clone, and a caller's own command runs
//! on one too. Underneath is one `redis` [`ConnectionManager`]: one multiplexed
//! socket, reopened behind its callers when Redis drops it.
//!
//! **Every command a caller waits on answers or fails within the connect
//! budget**, end to end — the wait for a reopened connection included, which a
//! reply timeout inside the client would never cover. A failure Redis reports —
//! a dropped connection, a refusal — arrives when it happens; the budget running
//! out arrives as a timeout (`redis::RedisError::is_timeout`). Without the bound
//! an outage held every caller: the rate limiter's request, a push, and every
//! loop apalis runs, for as long as the client kept reopening.
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig};
use redis::{Cmd, ConnectionAddr, IntoConnectionInfo, Pipeline, RedisFuture, Value};

use crate::error::RedisError;
use crate::{RedisConfig, tls};

/// The sentence every binding's boot error appends when the connection is
/// missing — one wording, three sites, so the remedy cannot drift.
pub(crate) const CONNECTION_REMEDY: &str = "RedisConnection is not registered — import \
     RedisModule::for_root(None), which opens the one Redis connection every Redis binding shares";

/// The app's shared Redis connection, and a connection in its own right: it
/// implements [`ConnectionLike`], so a clone runs any `redis` command, script or
/// pipeline — pass `&mut` the clone to `query_async` or `invoke_async`. A clone
/// shares the one underlying socket, so every binding and every caller reaches
/// Redis through it, never a second one.
///
/// A command answers or fails within the connect budget; when the budget is
/// what ends it, the error is a timeout (`redis::RedisError::is_timeout`).
///
/// Every holder multiplexes over one socket and one session — apalis and the
/// rate limiter included. Redis answers one connection's commands in order, so a
/// **blocking command** (`BLPOP`, `WAIT`, a `SUBSCRIBE`) stalls every holder for
/// as long as it blocks, and fails at the budget besides; and a **command that
/// changes the session** (`SELECT`, `WATCH`, a `MULTI` sent outside an atomic
/// pipeline, `CLIENT SETNAME`, `AUTH`) changes it for every holder. Open a
/// `redis::Client` of your own for those. Non-blocking, atomic operations (a
/// `Script`, `INCR`, `GET`/`SET`, an atomic pipeline) are the intended traffic.
#[derive(Clone)]
pub struct RedisConnection {
    manager: ConnectionManager,
    budget: Duration,
    /// TLS refusals met reopening the connection — `None` over plaintext, where
    /// no handshake can be refused.
    refusals: Option<Arc<TlsRefusals>>,
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
/// command fails. The first command failing with the shape a refused handshake
/// leaves therefore starts one handshake of the connection's own, whose error
/// still carries rustls's reason, and a refusal it meets is reported at `warn`
/// with what to change — once, until a command answers again.
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
            Err(error) if tls::handshake_shaped(error) => self.diagnose(),
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

    /// One handshake at a time, and none while the refusal stands reported. It
    /// runs beside the command that asked for it, which has failed already and
    /// must not wait on a second handshake past its budget.
    ///
    /// It carries no unit of work's trace, deliberately: the refusal is the
    /// connection's, met by every holder alike, and the command that happened to
    /// meet it first — a request, a job, one of apalis's own loops — is not the
    /// one it belongs to, any more than the boot's connection lines are.
    fn diagnose(self: &Arc<Self>) {
        if self.reported.load(Ordering::Relaxed) || self.diagnosing.swap(true, Ordering::Relaxed) {
            return;
        }
        let refusals = Arc::clone(self);
        tokio::spawn(async move {
            let attempt = tokio::time::timeout(
                refusals.budget,
                refusals.client.get_multiplexed_async_connection(),
            )
            .await;
            if let Ok(Err(error)) = attempt
                && tls::negotiation_failed(&error)
            {
                refusals.report(&error);
            }
            refusals.diagnosing.store(false, Ordering::Relaxed);
        });
    }
}

/// Backoff before the first retry; doubles up to [`MAX_RETRY_BACKOFF`] and is
/// always clamped to what is left of the budget.
const FIRST_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Ceiling for the doubling backoff, in the milliseconds `redis` takes it in.
const MAX_RETRY_BACKOFF_MS: u64 = 2_000;

/// Ceiling for the doubling backoff — a whole boot budget must still fit
/// several attempts, each of which gets its own `warn`. The connection's own
/// reconnect backoff takes the same ceiling.
const MAX_RETRY_BACKOFF: Duration = Duration::from_millis(MAX_RETRY_BACKOFF_MS);

/// How much each reconnect attempt's wait grows over the last. `redis`
/// defaults to a hundredfold, from one second, which after two failures is a
/// minute between attempts — a Redis back after a restart then waits out that
/// minute before a single caller is answered.
const RECONNECT_FACTOR: u64 = 2;

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
    /// change: a URL the client cannot parse; TLS settings it will not use —
    /// `#insecure`, material beside a plaintext URL, material no handshake
    /// could use; a handshake that fails the same way every time; and an answer
    /// Redis gives every time — refused credentials, an ACL denying the proof, a
    /// database index out of range. What may clear — a refused or reset TCP
    /// connection, a server still loading its dataset — is retried within the
    /// budget.
    ///
    /// A handshake nobody answers is not among them, because it cannot be told
    /// apart from a network that drops it: a `rediss://` URL pointed at a
    /// plaintext Redis runs out the budget as an unreachable one does.
    pub async fn connect(config: &RedisConfig) -> Result<Self, RedisError> {
        let budget = config.connect_timeout;
        let endpoint = address(&config.url);
        let client = client(config, &endpoint)?;
        let deadline = deadline_after(budget);
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
                        client.get_connection_info().addr,
                        ConnectionAddr::TcpTls { .. }
                    )
                    .then(|| {
                        Arc::new(TlsRefusals {
                            client,
                            endpoint,
                            budget,
                            reported: AtomicBool::new(false),
                            diagnosing: AtomicBool::new(false),
                        })
                    });
                    return Ok(Self {
                        manager,
                        budget,
                        refusals,
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

        Err(RedisError::Unreachable {
            endpoint,
            budget,
            attempts,
            source: last_error,
        })
    }
}

impl ConnectionLike for RedisConnection {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        let budget = self.budget;
        let refusals = self.refusals.as_ref();
        let call = self.manager.send_packed_command(cmd);
        Box::pin(async move {
            let outcome = bounded(budget, call).await?;
            if let Some(refusals) = refusals {
                refusals.observe(&outcome);
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
        let refusals = self.refusals.as_ref();
        let call = self.manager.send_packed_commands(pipeline, offset, count);
        Box::pin(async move {
            let outcome = bounded(budget, call).await?;
            if let Some(refusals) = refusals {
                refusals.observe(&outcome);
            }
            outcome
        })
    }

    fn get_db(&self) -> i64 {
        self.manager.get_db()
    }
}

/// `call`, or once `budget` elapses the timeout redis itself reports, so
/// `redis::RedisError::is_timeout` reads both the same way.
async fn bounded<F: Future>(budget: Duration, call: F) -> Result<F::Output, redis::RedisError> {
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

/// `budget` from now, or a year from now when the budget is too large for the
/// clock: a wait nobody means literally, which must not panic the boot.
fn deadline_after(budget: Duration) -> Instant {
    const FAR: Duration = Duration::from_secs(365 * 24 * 60 * 60);
    let now = Instant::now();
    now.checked_add(budget).unwrap_or_else(|| now + FAR)
}

/// Whether Redis answered in a way every attempt will repeat: what redis itself
/// marks as not worth retrying — a refused `SELECT`, an ACL `NOPERM`, an error
/// reply to the proof, a socket the process may not open — and refused
/// credentials, which it retries only on a new connection. A transport failure,
/// or a server still loading its dataset, is worth the budget and stays in it.
fn refused(error: &redis::RedisError) -> bool {
    error.kind() == redis::ErrorKind::AuthenticationFailed
        || matches!(error.retry_method(), redis::RetryMethod::NoRetry)
}

/// The client the connection is opened from, and reopened from, so the URL's
/// TLS and the material beside it reach every connection it opens. Nothing is
/// dialled: each refusal here is one every attempt would repeat.
fn client(config: &RedisConfig, endpoint: &str) -> Result<redis::Client, RedisError> {
    let invalid_url = |source: redis::RedisError| RedisError::InvalidUrl {
        endpoint: endpoint.to_owned(),
        source,
    };
    let info = config
        .url
        .as_str()
        .into_connection_info()
        .map_err(invalid_url)?;
    let ConnectionAddr::TcpTls { host, insecure, .. } = &info.addr else {
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
async fn prove(
    client: &redis::Client,
    budget: Duration,
) -> Result<ConnectionManager, redis::RedisError> {
    let mut proof = client.get_multiplexed_async_connection().await?;
    redis::cmd("PING").query_async::<()>(&mut proof).await?;
    ConnectionManager::new_with_config(client.clone(), manager_config(budget)).await
}

/// How the kept connection reopens after Redis drops it: each attempt bounded
/// by the budget, as the boot's are, and the backoff between attempts doubling
/// to the boot's ceiling rather than `redis`'s hundredfold. The reply timeout
/// stays off, because [`RedisConnection`] bounds every command itself — the
/// wait for a reopened connection included, which that timeout would miss.
fn manager_config(budget: Duration) -> ConnectionManagerConfig {
    ConnectionManagerConfig::new()
        .set_connection_timeout(budget)
        .set_factor(RECONNECT_FACTOR)
        .set_max_delay(MAX_RETRY_BACKOFF_MS)
}

/// The endpoint the connect diagnostics name, in logs and in the boot error:
/// the address the client parsed from the URL — `host:port`, or a socket path —
/// and never the URL, which routinely embeds a password in its userinfo or its
/// query. The client's own parser decides, so what is shown is what is dialled;
/// a URL it cannot parse is named by its scheme alone, and by nothing at all
/// when what precedes `://` is not a scheme and may be a credential.
fn address(url: &str) -> String {
    match url.into_connection_info() {
        Ok(info) => info.addr.to_string(),
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

    const AUTHORITY: &[u8] = include_bytes!("../tests/e2e/fixtures/tls_ca.pem");
    const SERVER_CERT: &[u8] = include_bytes!("../tests/e2e/fixtures/tls_server.pem");
    const SERVER_KEY: &[u8] = include_bytes!("../tests/e2e/fixtures/tls_server.key.pem");
    const CLIENT_CERT: &[u8] = include_bytes!("../tests/e2e/fixtures/tls_client.pem");

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
        let config = config(url, Duration::from_secs(2));
        let endpoint = address(url);
        Arc::new(TlsRefusals {
            client: client(&config, &endpoint).expect("the URL opens a client"),
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

    /// An error with no handshake in it — a dropped connection, a timeout, an
    /// answer — sends no handshake after one, and reports nothing.
    #[tokio::test]
    async fn an_error_no_handshake_left_sends_no_handshake_to_learn_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let refusals = refusals("rediss://127.0.0.1:9/");
        refusals.observe(&connection_dropped());
        refusals.observe(&Err::<(), _>(redis::RedisError::from(
            std::io::Error::from(std::io::ErrorKind::TimedOut),
        )));
        refusals.observe(&Err::<(), _>(redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "ERR",
        ))));
        assert!(!refusals.diagnosing.load(Ordering::Relaxed));
        assert!(logs.find(crate::TARGET, REFUSED).is_empty());
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

    /// A budget too large for the clock must not panic the boot on the
    /// `Instant` it computes.
    #[test]
    fn a_budget_too_large_for_the_clock_still_yields_a_deadline() {
        let deadline = deadline_after(Duration::MAX);
        assert!(deadline > Instant::now() + Duration::from_secs(300 * 24 * 60 * 60));
    }

    /// Every retry costs the boot a backoff and the operator a line telling
    /// them to widen the budget, so an answer that repeats must not get one —
    /// and what may clear must.
    #[test]
    fn an_answer_redis_repeats_is_refused_and_what_may_clear_is_retried() {
        let credentials = redis::RedisError::from((
            redis::ErrorKind::AuthenticationFailed,
            "Password authentication failed",
        ));
        let database = redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "Redis server refused to switch database",
        ));
        let acl = redis::RedisError::from((
            redis::ErrorKind::ExtensionError,
            "NOPERM",
            "User default has no permissions to run the 'ping' command".to_owned(),
        ));
        let transport =
            redis::RedisError::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        let loading = redis::RedisError::from((
            redis::ErrorKind::BusyLoadingError,
            "Redis is loading the dataset in memory",
        ));
        assert!(refused(&credentials), "refused credentials fail at once");
        assert!(
            refused(&database),
            "a database index out of range fails at once"
        );
        assert!(refused(&acl), "an ACL denying the proof fails at once");
        assert!(!refused(&transport), "a refused TCP connection may clear");
        assert!(!refused(&loading), "a server still loading may clear");
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
