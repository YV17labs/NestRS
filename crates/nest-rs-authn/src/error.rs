//! Authentication failures, rendered as HTTP 401 challenges — and the one that
//! is no challenge, an identity store or provider that did not answer, as 503.
//!
//! The token-endpoint vocabulary an *issuing* app answers with is
//! `nest-rs-oauth-server`'s: this crate resolves who is calling, and a grant
//! refusal is not a credential verdict.

use std::time::Duration;

use poem::error::ResponseError;
use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};

use nest_rs_guards::NoBearerChallenge;

/// Hashing-level failure from [`hash_password`](crate::hash_password) /
/// [`verify_password`](crate::verify_password).
///
/// Only [`InvalidHash`](Self::InvalidHash) means a *stored* record is unusable,
/// which deserves its own `error` line rather than "wrong password".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    /// Argon2 refused to hash — rare, and an infrastructure signal (OOM, RNG
    /// failure) rather than anything about the password. The cause is kept:
    /// it is what tells an operator *which* of those it was.
    #[error("password hashing failed")]
    HashFailed(#[source] argon2::password_hash::Error),
    /// The stored string is not a hash this hasher can run: it did not parse
    /// as PHC, or names an algorithm, version or parameters it refuses. The
    /// cause says which.
    #[error("stored password hash is not usable")]
    InvalidHash(#[source] argon2::password_hash::Error),
}

/// What a caller is told when its credential could not be evaluated: the
/// identity store was unreachable ([`AuthError::Unavailable`]), or the strategy
/// asking it did not answer within
/// [`AUTHENTICATE_TIMEOUT`](crate::AUTHENTICATE_TIMEOUT).
pub(crate) const UNAVAILABLE: &str = "authentication unavailable";

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
    /// Strategy-specific or configuration failures. The message is for logs, not the client body.
    #[error("authentication failed: {0}")]
    Failed(String),
    /// An identity store or an identity provider could not be reached, or did
    /// not answer — an infrastructure failure, **not** a credential signal: the
    /// caller did nothing wrong. Rendered as **503**, with a `Retry-After` when
    /// `retry_after` is known (RFC 9110 §15.6.4), and logged at `error`; the
    /// detail is for logs, never the client body.
    #[error("authentication unavailable: {detail}")]
    Unavailable {
        /// What did not answer, and how — for the log.
        detail: String,
        /// How long until a retry may succeed, when the party that failed said
        /// so — an identity provider's own `Retry-After`.
        retry_after: Option<Duration>,
    },
}

/// A credential mismatch is an authentication failure: it folds into
/// [`AuthError::Failed`], carrying [`CredentialError`]'s opaque `"invalid
/// credentials"` text for logs (the client still sees the constant
/// `client_message`).
impl From<CredentialError> for AuthError {
    fn from(err: CredentialError) -> Self {
        Self::Failed(err.to_string())
    }
}

impl AuthError {
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
            Self::Failed(_) => "failed",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// The `Retry-After` an [`Unavailable`](Self::Unavailable) failure answers
    /// with, in whole seconds rounded up — `None` for every other failure, and
    /// for an unavailable one nobody gave a wait for.
    pub fn retry_after_secs(&self) -> Option<u32> {
        let Self::Unavailable {
            retry_after: Some(wait),
            ..
        } = self
        else {
            return None;
        };
        let whole = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        Some(u32::try_from(whole).unwrap_or(u32::MAX))
    }

    /// Message safe to return in an HTTP 401 body (no strategy/configuration detail).
    pub fn client_message(&self) -> &'static str {
        match self {
            Self::Failed(_) => "authentication failed",
            Self::MissingCredentials => "missing credentials",
            Self::Unavailable { .. } => UNAVAILABLE,
            _ => "invalid token",
        }
    }
}

impl AuthError {
    /// The wire rendering, logged once. Shared by [`IntoResponse`] (a handler
    /// returning the error directly) and [`ResponseError`] (a handler `?`-ing
    /// it), so both spellings put the same bytes and the same log line out.
    fn render(&self) -> Response {
        let body = self.client_message();
        // An infrastructure failure is a 503 logged at `error`, never a 401 challenge.
        if let Self::Unavailable { detail, .. } = self {
            tracing::error!(target: crate::TARGET, detail = %detail, "authentication unavailable");
            let mut response = Response::builder().status(StatusCode::SERVICE_UNAVAILABLE);
            if let Some(secs) = self.retry_after_secs() {
                response = response.header(header::RETRY_AFTER, secs);
            }
            return response.body(body);
        }
        if let Self::Failed(detail) = self {
            tracing::warn!(target: crate::TARGET, detail = %detail, "authentication failed");
        }
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, self.challenge())
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

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        self.render()
    }
}

/// `?`-propagation from a handler: a service returns the framework type and it
/// flows to the transport boundary without a `map_err` at every call site.
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

/// Same `?`-propagation for the login path. A password mismatch is not a
/// `Bearer` challenge: no `WWW-Authenticate` goes out, and [`NoBearerChallenge`]
/// keeps the resource-server interceptor from adding one at the edge.
impl ResponseError for CredentialError {
    fn status(&self) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }

    fn as_response(&self) -> Response {
        let mut response = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(self.to_string());
        response.extensions_mut().insert(NoBearerChallenge);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_error_display_does_not_leak_detail() {
        assert_eq!(CredentialError.to_string(), "invalid credentials");
    }

    /// An outage is RFC 9110 §15.6.4's `503`, never a `401` nor a `500`.
    #[test]
    fn unavailable_renders_503_with_the_known_wait_and_no_bearer_challenge() {
        let logs = nest_rs_testing::LogCapture::install();
        let resp = AuthError::Unavailable {
            detail: "store unreachable".into(),
            retry_after: None,
        }
        .into_response();
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
        let waited = AuthError::Unavailable {
            detail: "provider answered 503".into(),
            retry_after: Some(Duration::from_millis(2500)),
        }
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

        // The body withholds the detail, so the log is its one place.
        let event = logs
            .find("nest_rs::authn", "authentication unavailable")
            .into_iter()
            .next()
            .expect("the outage is logged");
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("detail")
                .is_some_and(|d| d.contains("store unreachable")),
            "the event carries the detail the body withholds, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn a_failed_authentication_is_a_warn_and_not_this_line() {
        // An outage filed under this message would read as a brute-force attempt.
        let logs = nest_rs_testing::LogCapture::install();
        let resp = AuthError::Failed("bad password".into()).into_response();
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
        assert_eq!(
            AuthError::Unavailable {
                detail: "connection refused at 10.0.0.1".into(),
                retry_after: None,
            }
            .client_message(),
            "authentication unavailable",
        );
    }
}
