//! Authentication failures, rendered as HTTP 401 challenges.
//!
//! The token-endpoint vocabulary an *issuing* app answers with is
//! `nest-rs-oauth-server`'s: this crate resolves who is calling, and a grant
//! refusal is not a credential verdict.

use poem::error::ResponseError;
use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};

use nest_rs_guards::NoBearerChallenge;

/// Hashing-level failure from [`hash_password`](crate::hash_password) /
/// [`verify_password`](crate::verify_password).
///
/// Distinct from [`CredentialError`] on purpose: a login service collapses both
/// variants into that opaque type so the wire stays uniform, but only
/// [`InvalidHash`](Self::InvalidHash) means a *stored* record is unusable —
/// schema drift or corruption — and that one deserves its own `error` line
/// rather than being folded into "wrong password".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    /// Argon2 refused to hash — rare, and an infrastructure signal (OOM, RNG
    /// failure) rather than anything about the password.
    #[error("password hashing failed")]
    HashFailed,
    /// The stored string did not parse as a PHC hash.
    #[error("stored password hash is not valid PHC")]
    InvalidHash,
}

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
    /// The identity store was unreachable while authenticating — an
    /// infrastructure failure, **not** a credential signal. Rendered as
    /// **500** and logged at `error`; the message is for logs, never the
    /// client body. Kept distinct from [`Failed`](Self::Failed) so a backend
    /// outage during login is never reported to the caller as a 401.
    #[error("authentication unavailable: {0}")]
    Unavailable(String),
}

/// A credential mismatch is an authentication failure: it folds into
/// [`AuthError::Failed`], carrying [`CredentialError`]'s opaque `"invalid
/// credentials"` text for logs (the client still sees the constant
/// `client_message`). One conversion so the wire string lives in a single place.
impl From<CredentialError> for AuthError {
    fn from(err: CredentialError) -> Self {
        Self::Failed(err.to_string())
    }
}

impl AuthError {
    /// The RFC 6750 §3.1 code this failure reports on the `WWW-Authenticate`
    /// challenge, or `None` when §3 says to report none.
    ///
    /// §3 defines the `error` parameter as describing *why the request was
    /// declined*, and states it is omitted when the request carried no
    /// authentication at all — a challenge to a caller who presented nothing is
    /// an invitation, not a report. Everything else here is a token that was
    /// presented and refused, which is §3.1's `invalid_token`.
    ///
    /// Without it a client cannot tell "refresh and retry" from "start
    /// discovery", and both answers look like a bare `401`.
    pub fn error_code(&self) -> Option<&'static str> {
        match self {
            // Nothing was presented, so there is nothing to report on.
            AuthError::MissingCredentials => None,
            // Not a credential signal at all — a 500 carries no challenge.
            AuthError::Unavailable(_) => None,
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
            Self::NotYetValid => "not_yet_valid",
            Self::Expired => "expired",
            Self::Failed(_) => "failed",
            Self::Unavailable(_) => "unavailable",
        }
    }

    /// Message safe to return in an HTTP 401 body (no strategy/configuration detail).
    pub fn client_message(&self) -> &'static str {
        match self {
            Self::Failed(_) => "authentication failed",
            Self::MissingCredentials => "missing credentials",
            Self::Unavailable(_) => "authentication unavailable",
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
        // An infrastructure failure is a 500, logged at `error` — not a 401
        // challenge; the caller cannot fix it by re-authenticating.
        if let Self::Unavailable(detail) = self {
            tracing::error!(target: crate::TARGET, detail = %detail, "authentication unavailable");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(body);
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
    /// RFC 9110 §11.6.1 requires a `401` to carry a challenge at all, and RFC
    /// 6750 §3.1 supplies the code when a credential was actually presented and
    /// refused. A caller who sent nothing gets the bare scheme — §3: "if the
    /// request lacks any authentication information … the resource server
    /// SHOULD NOT include an error code".
    ///
    /// Built through `nest_rs_http::challenge` rather than a local `format!`:
    /// the grammar is one RFC production and three hand-written spellings of it
    /// had already drifted into three parameter sets for one failure.
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
            Self::Unavailable(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNAUTHORIZED,
        }
    }

    fn as_response(&self) -> Response {
        self.render()
    }
}

/// Same `?`-propagation for the login path, which returns the opaque
/// credential rejection rather than a token failure. A password mismatch is not
/// a `Bearer` challenge, so no `WWW-Authenticate` header goes out — and the
/// [`NoBearerChallenge`] marker keeps the resource-server interceptor from
/// adding one at the edge, where it can no longer tell the two kinds of `401`
/// apart. `POST /auth/login` is not a protected resource; pointing that caller
/// at an authorization server would be misdirection, not discovery.
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

    #[test]
    fn unavailable_renders_500_and_no_bearer_challenge() {
        let logs = nest_rs_testing::LogCapture::install();
        let resp = AuthError::Unavailable("store unreachable".into()).into_response();
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "an infrastructure failure is a 500, not a 401",
        );
        assert!(
            resp.headers().get(header::WWW_AUTHENTICATE).is_none(),
            "a 500 must not send a Bearer challenge the caller cannot satisfy",
        );

        // The client is told "authentication unavailable" and nothing else, on
        // purpose — which host is down and why is infrastructure detail. So the
        // detail exists in exactly one place, and it is the place an operator
        // looks when every login in the deployment starts answering 500.
        let event = logs.expect_one("nest_rs::authn", "authentication unavailable");
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
        // The neighbouring branch, and why they are two: a wrong password is a
        // caller problem answered `401`, an unreachable store is the
        // deployment's answered `500`. Filed under one message, an outage would
        // be indistinguishable from a brute-force attempt.
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
            AuthError::Unavailable("connection refused at 10.0.0.1".into()).client_message(),
            "authentication unavailable",
        );
    }
}
