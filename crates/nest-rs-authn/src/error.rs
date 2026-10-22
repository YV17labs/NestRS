//! Authentication failures, rendered as HTTP 401 challenges — and the one that
//! is no challenge, an identity store or provider that did not answer, as 503.
//!
//! The token-endpoint vocabulary an *issuing* app answers with is
//! `nest-rs-oauth-server`'s: this crate resolves who is calling, and a grant
//! refusal is not a credential verdict.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::__private::OpaqueCause;
use nest_rs_http::{HeaderValue, IntoResponse, Response, ResponseError, StatusCode, header};

use nest_rs_http::challenge::NoBearerChallenge;

/// Hashing-level failure from [`hash_password`](crate::hash_password) /
/// [`verify_password`](crate::verify_password).
///
/// Only [`InvalidHash`](Self::InvalidHash) means a *stored* record is unusable,
/// which deserves its own `error` line rather than "wrong password". Each
/// variant carries the hasher's error as a shared source: read it through
/// [`source`](Error::source), which downcasts to the hasher's own type.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum PasswordError {
    /// Argon2 refused to hash — rare, and an infrastructure signal (OOM, RNG
    /// failure) rather than anything about the password. The cause is kept:
    /// it is what tells an operator *which* of those it was.
    HashFailed(Arc<dyn Error + Send + Sync>),
    /// The stored string is not a hash this hasher can run: it did not parse
    /// as PHC, or names an algorithm, version or parameters it refuses. The
    /// cause says which.
    InvalidHash(Arc<dyn Error + Send + Sync>),
}

impl PasswordError {
    /// The hasher's error, shared so the error stays `Clone`.
    pub(crate) fn hash_failed(source: argon2::password_hash::Error) -> Self {
        Self::HashFailed(Arc::new(source))
    }

    /// The hasher's refusal of a stored record.
    pub(crate) fn invalid_hash(source: argon2::password_hash::Error) -> Self {
        Self::InvalidHash(Arc::new(source))
    }
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::HashFailed(_) => "password hashing failed",
            Self::InvalidHash(_) => "stored password hash is not usable",
        })
    }
}

// Written out: thiserror's `#[source]` would hand back the `Arc` itself, which
// no caller downcasts to the hasher's error through.
impl Error for PasswordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::HashFailed(source) | Self::InvalidHash(source) => Some(source.as_ref()),
        }
    }
}

/// A name that is not a JWS `alg` this verifier runs, refused by
/// [`Algorithm`](crate::Algorithm)'s `FromStr`: `none`, a name in the wrong
/// case, or an algorithm the verifier does not run. It never repeats the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a JWS algorithm this verifier runs")]
#[non_exhaustive]
pub struct UnknownAlgorithmError;

/// What a caller is told when its credential could not be evaluated: the
/// identity store was unreachable ([`AuthError::Unavailable`]), or the strategy
/// asking it did not answer within
/// [`AUTHENTICATE_TIMEOUT`](crate::AUTHENTICATE_TIMEOUT).
pub(crate) const UNAVAILABLE: &str = "authentication unavailable";

/// What a caller is told when a strategy or its configuration failed
/// ([`AuthError::Failed`]).
const FAILED: &str = "authentication failed";

/// Opaque "wrong credentials" failure for any password-login path.
///
/// Returned by services that verify a password against a stored hash: missing
/// user, missing hash, wrong password, and DB unreachable all collapse into
/// this single variant so timing and wire string never distinguish them.
/// `Display` is the fixed `"invalid credentials"`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid credentials")]
pub struct CredentialError;

/// Why authentication did not establish an identity.
///
/// The two internal-failure variants, [`Failed`](Self::Failed) and
/// [`Unavailable`](Self::Unavailable), display the constant the caller is
/// answered and keep their detail, then what failed, as their
/// [`source`](Error::source): the operator's line, filed with
/// [`error_message`](fn@nest_rs_core::error_message), says them together, and
/// no edge answers anything they carry. Only their constructors build them;
/// a match names them with `{ .. }`:
///
/// ```
/// use nest_rs_authn::AuthError;
///
/// let err = AuthError::failed("the provider answered 400");
/// assert!(matches!(err, AuthError::Failed { .. }));
/// ```
///
/// ```compile_fail
/// use nest_rs_authn::AuthError;
///
/// let err = AuthError::Failed {
///     cause: nest_rs_core::__private::OpaqueCause::new("the provider answered 400", None),
/// };
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    /// No credential was presented (no bearer token, no cookie). Rendered as a
    /// 401 challenge.
    #[error("missing credentials")]
    MissingCredentials,
    /// The token was malformed or failed a claim check (`aud`/`iss`/generic
    /// decode) — the catch-all token rejection.
    #[error("invalid token")]
    InvalidToken,
    /// The token's signature did not verify against the configured key.
    #[error("invalid token signature")]
    InvalidSignature,
    /// The token was signed with an algorithm the verifier does not accept —
    /// closes an `alg`-confusion downgrade.
    #[error("invalid token algorithm")]
    InvalidAlgorithm,
    /// No key of the issuer's JWK Set can check the token: the key its `kid`
    /// names is not in the set, even after the refresh an unknown key earns,
    /// or it names none and several keys fit its algorithm.
    #[error("unknown token signing key")]
    UnknownKey,
    /// The token's `nbf` (not-before) is still in the future, beyond leeway.
    #[error("token not yet valid")]
    NotYetValid,
    /// The token's `exp` has passed, beyond leeway. Kept distinct from the
    /// other token failures because it is the routine "log in again" case.
    #[error("token expired")]
    Expired,
    /// A strategy-specific or configuration failure. Rendered as a 401
    /// challenge and logged at `warn`; it displays `"authentication failed"`,
    /// the body every caller reads. Built by [`failed`](Self::failed) or, with
    /// what failed, [`failed_because`](Self::failed_because).
    #[error("{}", FAILED)]
    #[non_exhaustive]
    Failed {
        /// The detail, then the cause's chain, read through [`Error::source`].
        #[doc(hidden)]
        #[source]
        cause: OpaqueCause,
    },
    /// An identity store or an identity provider could not be reached, or did
    /// not answer — an infrastructure failure, **not** a credential signal: the
    /// caller did nothing wrong. Rendered as **503**, with a `Retry-After` when
    /// `retry_after` is known (RFC 9110 §15.6.4); the guard meeting it files
    /// it at `error`, and the HTTP edge files one a handler answers at `warn`,
    /// as every `503`. It displays `"authentication unavailable"`, the body
    /// every caller reads.
    /// Built by [`unavailable`](Self::unavailable) or, with what failed,
    /// [`unavailable_because`](Self::unavailable_because).
    #[error("{}", UNAVAILABLE)]
    #[non_exhaustive]
    Unavailable {
        /// How long until a retry may succeed, when the party that failed said
        /// so — an identity provider's own `Retry-After`.
        retry_after: Option<Duration>,
        /// The detail, then the cause's chain, read through [`Error::source`].
        #[doc(hidden)]
        #[source]
        cause: OpaqueCause,
    },
}

/// A credential mismatch is an authentication failure: it folds into
/// [`AuthError::Failed`] with [`CredentialError`]'s opaque `"invalid
/// credentials"` as its detail, and no cause — it carries none.
impl From<CredentialError> for AuthError {
    fn from(err: CredentialError) -> Self {
        Self::failed(err.to_string())
    }
}

impl AuthError {
    /// A [`Failed`](Self::Failed) authentication: the detail names what
    /// failed for the operator's line, never a value the request carried, and
    /// the caller reads `"authentication failed"` whatever it says.
    pub fn failed(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::Failed {
            cause: OpaqueCause::new(detail, None),
        }
    }

    /// [`failed`](Self::failed), keeping what failed beneath the detail: the
    /// operator's line says the detail, then the cause's chain. A source never
    /// quotes a value the request carried — a decode failure is said through
    /// its [`DecodeError`](nest_rs_core::DecodeError), a URL through
    /// [`endpoint_shown`](nest_rs_config::endpoint_shown) or reqwest's
    /// `without_url`, any other error as it displays.
    ///
    /// ```
    /// use nest_rs_authn::AuthError;
    ///
    /// let refused = std::io::Error::other("the key store refused the connection");
    /// let err = AuthError::failed_because("could not build the provider client", refused);
    /// assert_eq!(err.to_string(), "authentication failed");
    /// assert_eq!(
    ///     nest_rs_core::error_message(&err),
    ///     "authentication failed: could not build the provider client: the key store refused \
    ///      the connection",
    /// );
    /// ```
    pub fn failed_because(
        detail: impl Into<Cow<'static, str>>,
        source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
    ) -> Self {
        Self::Failed {
            cause: OpaqueCause::new(detail, Some(shared(source))),
        }
    }

    /// An [`Unavailable`](Self::Unavailable) identity store or provider, with
    /// the wait it gave when it gave one: the detail names what did not
    /// answer, for the operator's line, and the caller reads `"authentication
    /// unavailable"`.
    pub fn unavailable(
        detail: impl Into<Cow<'static, str>>,
        retry_after: Option<Duration>,
    ) -> Self {
        Self::Unavailable {
            retry_after,
            cause: OpaqueCause::new(detail, None),
        }
    }

    /// [`unavailable`](Self::unavailable), keeping what failed beneath the
    /// detail, under [`failed_because`](Self::failed_because)'s rule: a source
    /// never quotes a value the request carried.
    pub fn unavailable_because(
        detail: impl Into<Cow<'static, str>>,
        retry_after: Option<Duration>,
        source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
    ) -> Self {
        Self::Unavailable {
            retry_after,
            cause: OpaqueCause::new(detail, Some(shared(source))),
        }
    }

    /// The RFC 6750 §3.1 code this failure reports on the `WWW-Authenticate`
    /// challenge, or `None` when §3 says to report none.
    ///
    /// A caller who presented nothing gets no code (§3); a token presented and
    /// refused is §3.1's `invalid_token`.
    pub fn error_code(&self) -> Option<&'static str> {
        match self {
            // Nothing was presented, so there is nothing to report on.
            AuthError::MissingCredentials => None,
            // Not a credential signal at all — a 503 carries no challenge.
            AuthError::Unavailable { .. } => None,
            _ => Some(nest_rs_http::challenge::INVALID_TOKEN),
        }
    }

    /// Stable, low-cardinality code for the `reason` field of a security log —
    /// what an incident query groups on. Distinct from
    /// [`client_message`](Self::client_message), which is what the *caller*
    /// sees: the wire stays opaque, the log stays specific.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::MissingCredentials => "missing_credentials",
            Self::InvalidToken => "invalid_token",
            Self::InvalidSignature => "invalid_signature",
            Self::InvalidAlgorithm => "invalid_algorithm",
            Self::UnknownKey => "unknown_key",
            Self::NotYetValid => "not_yet_valid",
            Self::Expired => "expired",
            Self::Failed { .. } => "failed",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// The `Retry-After` an [`Unavailable`](Self::Unavailable) failure answers
    /// with, in whole seconds rounded up and never `0`, which RFC 9110
    /// §10.2.3 reads as "retry immediately" — `None` for every other failure,
    /// and for an unavailable one nobody gave a wait for.
    pub fn retry_after_secs(&self) -> Option<u32> {
        match self {
            Self::Unavailable {
                retry_after: Some(wait),
                ..
            } => Some(nest_rs_core::__private::retry_after_secs(*wait)),
            _ => None,
        }
    }

    /// Message safe to return in an HTTP 401 body (no strategy/configuration detail).
    pub fn client_message(&self) -> &'static str {
        match self {
            Self::Failed { .. } => FAILED,
            Self::MissingCredentials => "missing credentials",
            Self::Unavailable { .. } => UNAVAILABLE,
            _ => "invalid token",
        }
    }
}

/// `source` shared, its chain kept readable link by link.
fn shared(
    source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
) -> Arc<dyn Error + Send + Sync> {
    Arc::from(nest_rs_core::boxed_error(source))
}

impl AuthError {
    /// The wire rendering, shared by [`IntoResponse`] (a handler returning the
    /// error directly) and [`ResponseError`] (a handler `?`-ing it), so both
    /// spellings put the same bytes out. A refused credential's line is filed
    /// here, since no edge files a `401`; an outage's chain is filed by
    /// whoever holds the error last.
    fn render(&self) -> Response {
        let body = self.client_message();
        // An infrastructure failure is a 503, never a 401 challenge.
        if let Self::Unavailable { .. } = self {
            let mut response = Response::builder().status(StatusCode::SERVICE_UNAVAILABLE);
            if let Some(secs) = self.retry_after_secs() {
                response = response.header(header::RETRY_AFTER, HeaderValue::from(secs));
            }
            return response.body(body);
        }
        if let Self::Failed { .. } = self {
            tracing::warn!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(self),
                "authentication failed",
            );
        }
        // The challenge is composed from constants, so it always encodes.
        let challenge = HeaderValue::try_from(self.challenge())
            .unwrap_or_else(|_| HeaderValue::from_static(nest_rs_http::challenge::BEARER));
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, challenge)
            .body(body)
    }

    /// The `WWW-Authenticate` value this failure answers with.
    ///
    /// RFC 9110 §11.6.1 requires a challenge on a `401`; RFC 6750 §3 omits the
    /// error code when the request carried no credential.
    fn challenge(&self) -> String {
        match self.error_code() {
            Some(code) => {
                nest_rs_http::challenge::bearer_error_described(code, self.client_message())
            }
            None => nest_rs_http::challenge::BEARER.to_owned(),
        }
    }
}

/// A ready response, which no edge reads an error from: an outage's chain is
/// filed here, at `warn`, as the edge files every `503`.
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        if let Self::Unavailable { .. } = self {
            tracing::warn!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(&self),
                "authentication unavailable",
            );
        }
        self.render()
    }
}

/// `?`-propagation from a handler: a service returns the framework type and it
/// flows to the transport boundary without a `map_err` at every call site,
/// where the edge files an outage's chain once the answer is final.
impl ResponseError for AuthError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::UNAUTHORIZED,
        }
    }

    fn as_response(&self) -> Response {
        self.render()
    }
}

/// Same `?`-propagation for the login path. A password mismatch is no call to
/// discover an authorization server: [`NoBearerChallenge`] keeps the
/// resource-server interceptor from adding its pointer, and the edge answers
/// the bare `Bearer` challenge RFC 9110 §11.6.1 owes every `401`.
impl ResponseError for CredentialError {
    fn status(&self) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }

    fn as_response(&self) -> Response {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .extension(NoBearerChallenge)
            .body(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_error_display_does_not_leak_detail() {
        assert_eq!(CredentialError.to_string(), "invalid credentials");
    }

    /// A refused credential carries no cause: its words are said once.
    #[test]
    fn a_credential_mismatch_folds_into_failed_without_a_cause() {
        let err = AuthError::from(CredentialError);
        assert!(matches!(err, AuthError::Failed { .. }));
        assert_eq!(
            nest_rs_core::error_message(&err),
            "authentication failed: invalid credentials"
        );
        assert!(err.source().and_then(Error::source).is_none());
    }

    /// The cause sits beneath the detail, read through `source`, and the line
    /// is the whole chain; the body stays the constant.
    #[tokio::test]
    async fn a_failed_authentication_keeps_its_cause_and_files_the_chain_at_warn() {
        let logs = nest_rs_testing::LogCapture::install();
        let err = AuthError::failed_because("x", std::io::Error::other("the store hung up"));
        let cause = err
            .source()
            .and_then(Error::source)
            .expect("the cause beneath the detail");
        assert!(cause.downcast_ref::<std::io::Error>().is_some());
        assert_eq!(err.to_string(), "authentication failed");

        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.into_body().into_string().await.expect("a body"),
            "authentication failed"
        );
        let filed = logs.expect_one("nest_rs::authn", "authentication failed");
        assert_eq!(filed.level, "warn");
        assert_eq!(
            filed.field("error").as_deref(),
            Some("authentication failed: x: the store hung up")
        );
        logs.expect_none("nest_rs::authn", "authentication unavailable");
    }

    /// An outage is RFC 9110 §15.6.4's `503`, never a `401` nor a `500`.
    #[test]
    fn unavailable_renders_503_with_the_known_wait_and_no_bearer_challenge() {
        let logs = nest_rs_testing::LogCapture::install();
        let resp = AuthError::unavailable("store unreachable", None).into_response();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "an infrastructure failure is a 503, not a 401",
        );
        assert!(
            resp.headers().get(header::RETRY_AFTER).is_none(),
            "no wait is invented when nobody gave one"
        );
        assert!(
            resp.headers().get(header::WWW_AUTHENTICATE).is_none(),
            "a 503 must not send a Bearer challenge the caller cannot satisfy",
        );
        let waited = AuthError::unavailable_because(
            "provider answered 503",
            Some(Duration::from_millis(2500)),
            std::io::Error::other("upstream overloaded"),
        )
        .into_response();
        assert_eq!(waited.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            waited
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("3"),
            "delay-seconds, rounded up",
        );

        // The body withholds the detail and no edge holds the error any more,
        // so this line is its one place: at `warn`, as every `503`.
        let filed: Vec<_> = logs
            .find("nest_rs::authn", "authentication unavailable")
            .into_iter()
            .map(|event| {
                (
                    event.level.clone(),
                    event.field("error").unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            filed,
            [
                (
                    "warn".to_owned(),
                    "authentication unavailable: store unreachable".to_owned()
                ),
                (
                    "warn".to_owned(),
                    "authentication unavailable: provider answered 503: upstream overloaded"
                        .to_owned()
                ),
            ],
        );
    }

    /// Rendered for the edge, which files the `503` once the answer is
    /// final, the outage files nothing of its own.
    #[test]
    fn an_outage_rendered_for_the_edge_files_nothing_of_its_own() {
        let logs = nest_rs_testing::LogCapture::install();
        let resp = AuthError::unavailable("store unreachable", None).as_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(logs.events().is_empty(), "{:#?}", logs.events());
    }

    /// RFC 9110 §10.2.3 reads `0` as "retry now": a provider's zero wait is
    /// answered as one second, the floor every `Retry-After` the framework
    /// sends holds.
    #[test]
    fn a_zero_wait_is_answered_as_one_second() {
        for (wait, secs) in [
            (Duration::ZERO, 1),
            (Duration::from_millis(1), 1),
            (Duration::from_secs(30), 30),
        ] {
            assert_eq!(
                AuthError::unavailable("provider answered 503", Some(wait)).retry_after_secs(),
                Some(secs),
                "{wait:?}"
            );
        }
        assert_eq!(AuthError::unavailable("x", None).retry_after_secs(), None);
        assert_eq!(AuthError::failed("x").retry_after_secs(), None);
    }

    #[test]
    fn a_failed_authentication_is_a_warn_and_not_this_line() {
        // An outage filed under this message would read as a brute-force attempt.
        let logs = nest_rs_testing::LogCapture::install();
        let resp = AuthError::failed("bad password").into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            logs.expect_one("nest_rs::authn", "authentication failed")
                .level,
            "warn"
        );
        logs.expect_none("nest_rs::authn", "authentication unavailable");
    }

    #[test]
    fn unavailable_client_message_hides_the_detail() {
        let err = AuthError::unavailable("connection refused at 10.0.0.1", None);
        assert_eq!(err.client_message(), "authentication unavailable");
        assert_eq!(err.to_string(), "authentication unavailable");
    }

    /// A decode failure beneath the detail is the operator's: no edge reading
    /// the chain for what to answer finds it, wrapped or not, and the line
    /// says it without its value.
    #[test]
    fn a_decode_failure_beneath_the_detail_is_never_the_callers() {
        let failure = serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("not a number");
        let err = anyhow::Error::from(AuthError::failed_because(
            "the provider answered a body that does not parse",
            failure,
        ))
        .context("resolving the caller");
        assert!(nest_rs_core::DecodeError::in_chain(err.as_ref()).is_none());
        let said = nest_rs_core::error_message(err.as_ref());
        assert!(said.contains("expected u64"), "{said}");
        assert!(!said.contains("sk_live"), "{said}");
    }
}
