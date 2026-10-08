//! [`Jwks`] — the JWK Set an external issuer publishes at
//! `<PREFIX>_AUTHN__JWKS_URI`, fetched over verified TLS and kept fresh.
//!
//! - **Fetched on first use, never at boot**: a resource server does not fail
//!   to start, or restart in a loop, because its issuer is briefly
//!   unreachable. What can be judged without the network — the URI, its
//!   scheme, the authorities — is judged at boot.
//! - **One fetch at a time, on a task of its own**: tokens arriving meanwhile
//!   wait for it, and a caller dropped mid-fetch cancels nothing.
//! - **At most one fetch per [`JWKS_REFRESH_FLOOR`]**, whatever asks for it — a
//!   token naming a `kid` the set lacks, a set gone stale, a failure retried —
//!   so no token makes this server flood its issuer.
//! - **Fresh for the answer's `Cache-Control: max-age`**, net of its `Age`,
//!   between the floor and [`JWKS_MAX_AGE_CEILING`]; [`JWKS_DEFAULT_MAX_AGE`]
//!   when it states none. A stale set keeps serving while its refresh runs.
//! - **A failed refresh keeps the last good set**, said at `warn`, for
//!   [`JWKS_STALE_CEILING`] past its freshness. Past it, or before any set was
//!   fetched, a token is [`AuthError::Unavailable`] — the `503` that never
//!   blames the token.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jsonwebtoken::{Algorithm, DecodingKey};
use reqwest::header::{ACCEPT, AGE, CACHE_CONTROL, HeaderMap, HeaderName, RETRY_AFTER};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::AuthnTls;
use crate::config::{jwks_uri_setting, tls_ca_cert_setting};
use crate::error::AuthError;
use crate::jwk_set::{JwkSet, Selection};
use crate::service::one_of;

/// How long reaching the JWK Set endpoint — name lookup, TCP and the TLS
/// handshake — may take before a fetch fails, naming the endpoint.
pub const JWKS_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long one fetch of the JWK Set may take in all, its body included. A
/// token waits on one fetch at most, so this stays well under
/// [`AUTHENTICATE_TIMEOUT`](crate::AUTHENTICATE_TIMEOUT).
pub const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// The largest JWK Set document read, in bytes; a longer answer is refused
/// without reading past it.
pub const JWKS_MAX_BYTES: usize = 128 * 1024;

/// The least time between the starts of two fetches, whatever asks for them,
/// and so the shortest a set stays fresh.
pub const JWKS_REFRESH_FLOOR: Duration = Duration::from_secs(30);

/// How long a set stays fresh when its answer states no `max-age`.
pub const JWKS_DEFAULT_MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// The longest a set stays fresh, whatever its `max-age` says: a key the issuer
/// withdrew stops verifying within it while the issuer answers.
pub const JWKS_MAX_AGE_CEILING: Duration = Duration::from_secs(60 * 60);

/// How long past its freshness a set still verifies while no refresh succeeds.
pub const JWKS_STALE_CEILING: Duration = Duration::from_secs(60 * 60);

/// An issuer's JWK Set, fetched from its URI and kept fresh.
pub(crate) struct Jwks {
    inner: Arc<Inner>,
}

struct Inner {
    url: reqwest::Url,
    /// The endpoint as a sentence names it: origin and path, never a query.
    endpoint: String,
    client: reqwest::Client,
    /// What a key of the set may verify, at most.
    algorithms: Vec<Algorithm>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    held: Option<Held>,
    /// Why the last fetch failed; `None` once one succeeds.
    failure: Option<Failure>,
    /// When the last fetch started, which the floor counts from.
    last_attempt: Option<Instant>,
    /// The last fetch started: still running while its sender lives.
    in_flight: Option<watch::Receiver<()>>,
}

struct Held {
    set: JwkSet,
    fresh_until: Instant,
}

struct Failure {
    detail: String,
    /// The issuer's own `Retry-After`, when it gave one in delay-seconds.
    retry_after: Option<Duration>,
}

/// What [`State::join`] found: the fetch to wait for, if any, and the sender of
/// one it started, which the caller spawns once the state is unlocked.
struct Joined {
    fetch: Option<watch::Receiver<()>>,
    started: Option<watch::Sender<()>>,
}

impl Jwks {
    /// The set published at `uri`, its endpoint's certificate chained to the
    /// authorities `tls` names or the system's. Nothing is fetched yet.
    ///
    /// Refuses a URI that does not parse or is not `https`, and an authority
    /// file holding no certificate, naming the setting.
    pub(crate) fn new(
        uri: &str,
        tls: &AuthnTls,
        algorithms: &[Algorithm],
    ) -> Result<Self, AuthError> {
        let url = reqwest::Url::parse(uri.trim()).map_err(|error| {
            AuthError::Failed(format!("{}, is not a URL: {error}", jwks_uri_setting()))
        })?;
        if url.scheme() != "https" {
            return Err(AuthError::Failed(format!(
                "{}, must be an https URL: the keys it serves decide which tokens verify, and \
                 over plain http anyone on the path could serve their own",
                jwks_uri_setting()
            )));
        }
        let roots = match &tls.ca_cert {
            Some(pem) => reqwest::Certificate::from_pem_bundle(pem)
                .ok()
                .filter(|found| !found.is_empty())
                .ok_or_else(|| {
                    AuthError::Failed(format!(
                        "{}, holds no PEM CERTIFICATE block, so it would trust no certificate",
                        tls_ca_cert_setting(),
                    ))
                })?,
            None => reqwest::Certificate::from_pem_bundle(nest_rs_config::system_authorities())
                .map_err(|error| {
                    AuthError::Failed(format!(
                        "the system's certificate store does not read as PEM: {}",
                        nest_rs_core::error_message(&error)
                    ))
                })?,
        };
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("nestrs")
            .connect_timeout(JWKS_CONNECT_TIMEOUT)
            .timeout(JWKS_FETCH_TIMEOUT)
            .tls_certs_only(roots)
            .build()
            .map_err(|error| {
                AuthError::Failed(format!(
                    "the JWK Set client could not be built: {}",
                    nest_rs_core::error_message(&error)
                ))
            })?;
        let endpoint = format!(
            "the JWK Set at {}{}",
            url.origin().ascii_serialization(),
            url.path()
        );
        Ok(Self {
            inner: Arc::new(Inner {
                url,
                endpoint,
                client,
                algorithms: algorithms.to_vec(),
                state: Mutex::new(State::default()),
            }),
        })
    }

    /// The key that checks a token signed with `algorithm` under `kid`.
    ///
    /// Fetches the set when none is held, or once more when it lacks the key
    /// the token names, within the floor. A set that answers without the key
    /// is [`AuthError::UnknownKey`]; a set that cannot be had — never fetched,
    /// past its stale ceiling, or failing as the token's key is looked for — is
    /// [`AuthError::Unavailable`].
    pub(crate) async fn key_for(
        &self,
        kid: Option<&str>,
        algorithm: Algorithm,
    ) -> Result<Arc<DecodingKey>, AuthError> {
        let inner = &self.inner;
        let (fetch, started) = {
            let now = Instant::now();
            let mut state = inner.lock();
            let mut held = false;
            if let Some(found) = state.usable(now) {
                held = true;
                let selection = found.set.select(kid, algorithm);
                if !matches!(selection, Selection::Unknown) {
                    let stale = now >= found.fresh_until;
                    let started = if stale { state.join(now).started } else { None };
                    drop(state);
                    if let Some(started) = started {
                        inner.spawn_fetch(started);
                    }
                    return selection.into_key();
                }
            }
            let joined = state.join(now);
            let Some(fetch) = joined.fetch else {
                return Err(if held {
                    state.unknown(&inner.endpoint, now)
                } else {
                    state.unavailable(&inner.endpoint, now)
                });
            };
            (fetch, joined.started)
        };
        if let Some(started) = started {
            inner.spawn_fetch(started);
        }
        settled(fetch).await;
        inner
            .lock()
            .answer(&inner.endpoint, Instant::now(), kid, algorithm)
    }
}

/// Wait for the fetch `fetch` follows to settle: its sender is dropped once it
/// has, and nothing is ever sent. Bounded by [`JWKS_FETCH_TIMEOUT`], which
/// bounds the fetch.
async fn settled(mut fetch: watch::Receiver<()>) {
    while fetch.changed().await.is_ok() {}
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Every write is a whole assignment, so a poisoned state is still whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run the fetch [`State::join`] started; dropping `started` once it has
    /// settled wakes every token waiting on it.
    fn spawn_fetch(self: &Arc<Self>, started: watch::Sender<()>) {
        let inner = Arc::clone(self);
        let task = async move {
            let outcome = inner.fetch().await;
            inner.settle(outcome);
            drop(started);
        };
        // The token that asked is the fetch's cause, so its lines carry the
        // token's trace — never its request scope, which ends without it.
        let handle = match nest_rs_core::current_correlation() {
            Some(correlation) => {
                tokio::spawn(nest_rs_core::with_request_scope(None, correlation, task))
            }
            None => tokio::spawn(task),
        };
        drop(handle);
    }

    /// One fetch of the set, bounded by [`JWKS_FETCH_TIMEOUT`] and
    /// [`JWKS_MAX_BYTES`]; a redirect is an answer like any other `3xx`.
    async fn fetch(&self) -> Result<(JwkSet, Duration), Failure> {
        let mut response = self
            .client
            .get(self.url.clone())
            .header(ACCEPT, "application/jwk-set+json, application/json")
            .send()
            .await
            .map_err(|error| self.call_failed(error))?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = (status.as_u16() == 429 || status.is_server_error())
                .then(|| delay_seconds(response.headers(), RETRY_AFTER))
                .flatten();
            let redirect = if status.is_redirection() {
                " — a redirect is not followed: set the URI it points to"
            } else {
                ""
            };
            return Err(Failure {
                detail: format!("{} answered {status}{redirect}", self.endpoint),
                retry_after,
            });
        }
        let fresh_for = freshness(response.headers());
        let oversized = || Failure {
            detail: format!(
                "{} answered more than {JWKS_MAX_BYTES} bytes, the most a JWK Set is read to",
                self.endpoint
            ),
            retry_after: None,
        };
        if response
            .content_length()
            .is_some_and(|length| length > JWKS_MAX_BYTES as u64)
        {
            return Err(oversized());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| self.call_failed(error))?
        {
            if body.len() + chunk.len() > JWKS_MAX_BYTES {
                return Err(oversized());
            }
            body.extend_from_slice(&chunk);
        }
        // Said without the value serde would quote.
        let set = JwkSet::parse(&body, &self.algorithms).map_err(|error| Failure {
            detail: format!(
                "{} answered a document that is not a JWK Set: {}",
                self.endpoint,
                nest_rs_core::DecodeError::new(&error)
            ),
            retry_after: None,
        })?;
        if set.len() == 0 {
            return Err(Failure {
                detail: format!(
                    "{} holds no key this verifier can use: none is a signing key for {}",
                    self.endpoint,
                    one_of(self.algorithms.iter().copied())
                ),
                retry_after: None,
            });
        }
        Ok((set, fresh_for))
    }

    /// A call that got no answer, without the URL reqwest quotes whole — its
    /// query may carry a credential.
    fn call_failed(&self, error: reqwest::Error) -> Failure {
        let detail = if error.is_timeout() && error.is_connect() {
            format!(
                "{} could not be connected to within {JWKS_CONNECT_TIMEOUT:?}",
                self.endpoint
            )
        } else if error.is_timeout() {
            format!(
                "{} did not answer within {JWKS_FETCH_TIMEOUT:?}",
                self.endpoint
            )
        } else {
            format!(
                "{} could not be fetched: {}",
                self.endpoint,
                nest_rs_core::error_message(&error.without_url())
            )
        };
        Failure {
            detail,
            retry_after: None,
        }
    }

    /// Record a fetch's outcome; a failure beside a set still served is said.
    fn settle(&self, outcome: Result<(JwkSet, Duration), Failure>) {
        let now = Instant::now();
        let mut state = self.lock();
        match outcome {
            Ok((set, fresh_for)) => {
                let (keys, skipped) = (set.len(), set.skipped());
                state.held = Some(Held {
                    set,
                    fresh_until: now + fresh_for,
                });
                state.failure = None;
                drop(state);
                tracing::debug!(
                    target: crate::TARGET,
                    jwks = %self.endpoint,
                    keys,
                    skipped,
                    fresh_secs = fresh_for.as_secs(),
                    "the issuer's key set was fetched",
                );
            }
            Err(failure) => {
                let kept = state.usable(now).is_some();
                let detail = failure.detail.clone();
                state.failure = Some(failure);
                drop(state);
                // With nothing kept, every token waiting answers `Unavailable`
                // and its guard says so.
                if kept {
                    tracing::warn!(
                        target: crate::TARGET,
                        error = %detail,
                        "the issuer's key set could not be refreshed; the last good one is kept",
                    );
                }
            }
        }
    }
}

impl State {
    fn usable(&self, now: Instant) -> Option<&Held> {
        self.held
            .as_ref()
            .filter(|held| now < held.fresh_until + JWKS_STALE_CEILING)
    }

    /// The fetch in flight, or a new one when the floor allows it — its
    /// sender in `started` — or neither, when a fetch started within the floor
    /// and has settled.
    fn join(&mut self, now: Instant) -> Joined {
        if let Some(fetch) = self
            .in_flight
            .as_ref()
            .filter(|fetch| fetch.has_changed().is_ok())
        {
            return Joined {
                fetch: Some(fetch.clone()),
                started: None,
            };
        }
        if self
            .last_attempt
            .is_some_and(|started| now.duration_since(started) < JWKS_REFRESH_FLOOR)
        {
            return Joined {
                fetch: None,
                started: None,
            };
        }
        let (started, fetch) = watch::channel(());
        self.last_attempt = Some(now);
        self.in_flight = Some(fetch.clone());
        Joined {
            fetch: Some(fetch),
            started: Some(started),
        }
    }

    /// What a token gets once the fetch it waited for has settled.
    fn answer(
        &self,
        endpoint: &str,
        now: Instant,
        kid: Option<&str>,
        algorithm: Algorithm,
    ) -> Result<Arc<DecodingKey>, AuthError> {
        let Some(held) = self.usable(now) else {
            return Err(self.unavailable(endpoint, now));
        };
        match held.set.select(kid, algorithm) {
            Selection::Unknown => Err(self.unknown(endpoint, now)),
            selection => selection.into_key(),
        }
    }

    /// A key the held set lacks: unknown when the last fetch answered, and
    /// unavailable when it failed — the key was never looked for.
    fn unknown(&self, endpoint: &str, now: Instant) -> AuthError {
        if self.failure.is_some() {
            self.unavailable(endpoint, now)
        } else {
            AuthError::UnknownKey
        }
    }

    /// The `503` a token gets while no set can be had: the last failure, and
    /// the issuer's wait or the time until the next fetch may start.
    fn unavailable(&self, endpoint: &str, now: Instant) -> AuthError {
        let next_fetch = self.last_attempt.map_or(Duration::ZERO, |started| {
            JWKS_REFRESH_FLOOR.saturating_sub(now.duration_since(started))
        });
        let next_fetch = (!next_fetch.is_zero()).then_some(next_fetch);
        match &self.failure {
            Some(failure) => AuthError::Unavailable {
                detail: failure.detail.clone(),
                retry_after: failure.retry_after.or(next_fetch),
            },
            None => AuthError::Unavailable {
                detail: format!("{endpoint} holds no set this verifier can use yet"),
                retry_after: next_fetch,
            },
        }
    }
}

/// How long an answer's set stays fresh: its `max-age`, net of its `Age`, held
/// between [`JWKS_REFRESH_FLOOR`] and [`JWKS_MAX_AGE_CEILING`] (RFC 9111 §4.2).
fn freshness(headers: &HeaderMap) -> Duration {
    let lifetime = max_age(headers).unwrap_or(JWKS_DEFAULT_MAX_AGE);
    let age = delay_seconds(headers, AGE).unwrap_or_default();
    lifetime
        .saturating_sub(age)
        .clamp(JWKS_REFRESH_FLOOR, JWKS_MAX_AGE_CEILING)
}

/// The first `max-age` the answer's `Cache-Control` states, or zero when it
/// says `no-store` or `no-cache`: the set is held either way, since every token
/// needs it, and checked again as soon as the floor allows.
fn max_age(headers: &HeaderMap) -> Option<Duration> {
    let mut stated = None;
    for value in headers.get_all(CACHE_CONTROL) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for directive in value.split(',') {
            let (name, argument) = match directive.split_once('=') {
                Some((name, argument)) => (name.trim(), Some(argument.trim().trim_matches('"'))),
                None => (directive.trim(), None),
            };
            if name.eq_ignore_ascii_case("no-store") || name.eq_ignore_ascii_case("no-cache") {
                return Some(Duration::ZERO);
            }
            if stated.is_none() && name.eq_ignore_ascii_case("max-age") {
                stated = argument.and_then(seconds);
            }
        }
    }
    stated
}

/// A `delay-seconds` header (`Retry-After`, `Age`); an HTTP-date would trust
/// the issuer's clock, and is not read.
fn delay_seconds(headers: &HeaderMap, name: HeaderName) -> Option<Duration> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| seconds(value.trim()))
}

/// RFC 9111 §1.2.2's `delay-seconds`: digits, a value past what fits read as
/// the largest.
fn seconds(digits: &str) -> Option<Duration> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Duration::from_secs(digits.parse().unwrap_or(u64::MAX)))
}

#[cfg(test)]
mod tests {
    use nest_rs_config::Namespaced;
    use reqwest::header::HeaderValue;

    use super::*;
    use crate::AuthnConfig;

    fn headers(pairs: &[(HeaderName, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(name, HeaderValue::from_str(value).expect("a header value"));
        }
        headers
    }

    /// A token waits on one fetch at most, and the guard's net must not be
    /// what answers it.
    #[test]
    fn a_fetch_ends_well_inside_the_guards_net() {
        assert!(JWKS_CONNECT_TIMEOUT < JWKS_FETCH_TIMEOUT);
        assert!(JWKS_FETCH_TIMEOUT * 2 < crate::AUTHENTICATE_TIMEOUT);
    }

    #[test]
    fn the_freshness_bounds_are_ordered() {
        assert!(JWKS_REFRESH_FLOOR < JWKS_DEFAULT_MAX_AGE);
        assert!(JWKS_DEFAULT_MAX_AGE < JWKS_MAX_AGE_CEILING);
    }

    #[test]
    fn max_age_is_honoured_between_the_floor_and_the_ceiling() {
        for (cache_control, fresh) in [
            ("public, max-age=600", Duration::from_secs(600)),
            ("max-age=\"120\", must-revalidate", Duration::from_secs(120)),
            ("MAX-AGE=5", JWKS_REFRESH_FLOOR),
            ("max-age=86400", JWKS_MAX_AGE_CEILING),
            ("max-age=99999999999999999999999", JWKS_MAX_AGE_CEILING),
            ("no-cache", JWKS_REFRESH_FLOOR),
            ("max-age=600, no-store", JWKS_REFRESH_FLOOR),
            ("max-age=-1", JWKS_DEFAULT_MAX_AGE),
            ("private", JWKS_DEFAULT_MAX_AGE),
        ] {
            assert_eq!(
                freshness(&headers(&[(CACHE_CONTROL, cache_control)])),
                fresh,
                "{cache_control}"
            );
        }
        assert_eq!(freshness(&HeaderMap::new()), JWKS_DEFAULT_MAX_AGE);
    }

    /// RFC 9111 §4.2.1: the first occurrence of a repeated directive counts.
    #[test]
    fn the_first_max_age_counts_across_header_lines() {
        let repeated = headers(&[
            (CACHE_CONTROL, "max-age=300"),
            (CACHE_CONTROL, "max-age=900"),
        ]);
        assert_eq!(freshness(&repeated), Duration::from_secs(300));
    }

    /// A cached copy a CDN already held is that much less fresh.
    #[test]
    fn the_age_of_a_cached_answer_is_taken_off() {
        let aged = headers(&[(CACHE_CONTROL, "max-age=600"), (AGE, "500")]);
        assert_eq!(freshness(&aged), Duration::from_secs(100));
        let spent = headers(&[(CACHE_CONTROL, "max-age=600"), (AGE, "900")]);
        assert_eq!(freshness(&spent), JWKS_REFRESH_FLOOR);
    }

    #[test]
    fn a_retry_after_is_read_in_delay_seconds_only() {
        let read = |value| delay_seconds(&headers(&[(RETRY_AFTER, value)]), RETRY_AFTER);
        assert_eq!(read("120"), Some(Duration::from_secs(120)));
        assert_eq!(read("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(read("-5"), None);
    }

    #[test]
    fn a_uri_that_is_not_https_or_not_a_uri_is_refused_naming_the_setting() {
        let setting = nest_rs_config::var_name(AuthnConfig::NAMESPACE, "JWKS_URI");
        for (uri, reason) in [
            ("http://issuer.example/jwks", "must be an https URL"),
            ("issuer.example/jwks", "is not a URL"),
            ("ftp://issuer.example/jwks", "must be an https URL"),
        ] {
            let Err(AuthError::Failed(refused)) =
                Jwks::new(uri, &AuthnTls::default(), &[Algorithm::EdDSA])
            else {
                panic!("{uri} must be refused");
            };
            assert!(
                refused.contains(&setting) && refused.contains(reason),
                "{refused}"
            );
        }
    }

    #[test]
    fn an_authority_file_holding_no_certificate_is_refused() {
        let setting = nest_rs_config::var_name(AuthnConfig::NAMESPACE, "TLS_CA_CERT");
        for pem in [&b""[..], b"not a certificate"] {
            let Err(AuthError::Failed(refused)) = Jwks::new(
                "https://issuer.example/jwks",
                &AuthnTls {
                    ca_cert: Some(pem.to_vec()),
                },
                &[Algorithm::EdDSA],
            ) else {
                panic!("an authority file with no certificate must be refused");
            };
            assert!(refused.contains(&setting), "{refused}");
        }
    }

    /// A sentence the endpoint appears in names its origin and path, never a
    /// query that may carry an API key.
    #[test]
    fn the_endpoint_is_named_without_its_query() {
        let jwks = Jwks::new(
            "https://issuer.example/keys?api_key=never-quoted",
            &AuthnTls::default(),
            &[Algorithm::EdDSA],
        )
        .expect("an https URI");
        assert_eq!(
            jwks.inner.endpoint,
            "the JWK Set at https://issuer.example/keys"
        );
    }
}
