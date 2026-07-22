//! Authentication failures, rendered as HTTP 401 challenges.

use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};

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
    pub fn client_message(&self) -> String {
        match self {
            Self::Failed(_) => "authentication failed".into(),
            Self::MissingCredentials => "missing credentials".into(),
            Self::Unavailable(_) => "authentication unavailable".into(),
            _ => "invalid token".into(),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = self.client_message();
        // An infrastructure failure is a 500, logged at `error` — not a 401
        // challenge; the caller cannot fix it by re-authenticating.
        if let Self::Unavailable(ref detail) = self {
            tracing::error!(target: "nest_rs::authn", detail = %detail, "authentication unavailable");
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(body);
        }
        if let Self::Failed(ref detail) = self {
            tracing::warn!(target: "nest_rs::authn", detail = %detail, "authentication failed");
        }
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Bearer")
            .body(body)
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
    }

    #[test]
    fn unavailable_client_message_hides_the_detail() {
        assert_eq!(
            AuthError::Unavailable("connection refused at 10.0.0.1".into()).client_message(),
            "authentication unavailable",
        );
    }
}
