//! RFC 6749 §5.2's token-endpoint error vocabulary, and the client
//! authentication (§2.3.1) an issuer performs before it mints anything.

use nest_rs_guards::NoBearerChallenge;
use poem::http::{StatusCode, header};
use poem::{IntoResponse, Response};

/// Token-endpoint failure — RFC 6749 §5.2's closed set, all six of it.
///
/// `Display` yields the wire code an OAuth2 client reads in the `error` member
/// of the JSON error response, so the variant names map to the spec rather than
/// to internal detail. A code outside §5.2's set is one no conforming client can
/// branch on.
///
/// **One half of §5.2 is deliberately not built here.** The clause "include the
/// `WWW-Authenticate` response header field matching the authentication scheme
/// used by the client" needs the scheme the client actually presented, and this
/// enum does not carry it — a `401` reaches us the same way whether the client
/// sent `Authorization: Basic` or form-encoded credentials in the body.
/// What is built instead is the negative half: the response is marked
/// [`NoBearerChallenge`] so nothing downstream stamps a `Bearer` challenge onto
/// it, which is what a `Basic`-authenticating client used to receive. Threading
/// the scheme through is an owner question, not a refusal — §5.2 permits it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TokenError {
    /// The request is malformed — a required parameter missing, repeated or
    /// otherwise unparseable (400).
    #[error("invalid_request")]
    InvalidRequest,
    /// The requested `grant_type` is not one this endpoint serves (400).
    #[error("unsupported_grant_type")]
    UnsupportedGrant,
    /// The grant itself is invalid, expired, revoked, or was issued to another
    /// client (400) — **and this is the code a rejected resource-owner password
    /// takes**. RFC 6749 §5.2 assigns "resource owner credentials … are
    /// invalid" here, and reserves [`InvalidClient`](Self::InvalidClient) for a
    /// failure of the *client's* own authentication. Reporting a wrong password
    /// as `invalid_client` tells the client its registration is broken and
    /// sends it to re-check credentials that are fine.
    #[error("invalid_grant")]
    InvalidGrant,
    /// This client is not authorized to use the requested grant type (400) —
    /// the client authenticated, and the grant is one its registration does not
    /// permit. Distinct from [`InvalidGrant`](Self::InvalidGrant), which is
    /// about the grant *value*, and from
    /// [`UnsupportedGrant`](Self::UnsupportedGrant), which is about the
    /// endpoint not serving that type to anyone.
    #[error("unauthorized_client")]
    UnauthorizedClient,
    /// The requested scope is unknown or not permitted for this client (400).
    #[error("invalid_scope")]
    InvalidScope,
    /// Client authentication failed — unknown client, no client authentication
    /// included, or an unsupported authentication method (401). RFC 6749 §5.2
    /// names this condition `invalid_client`; the set of codes is closed, so a
    /// spelling of our own is one no conforming client can branch on.
    #[error("invalid_client")]
    InvalidClient,
    /// Internal signing failure. `Display` is the opaque RFC 6749
    /// `server_error`; the source stays attached for `tracing`.
    #[error("server_error")]
    Sign(#[source] anyhow::Error),
    /// A backend dependency (e.g. the identity store) was unreachable while
    /// resolving the grant — distinct from a credential rejection. `Display`
    /// is the opaque RFC 6749 `server_error`; the source stays attached for
    /// `tracing`.
    #[error("server_error")]
    Server(#[source] anyhow::Error),
}

impl TokenError {
    /// The HTTP status RFC 6749 §5.2 assigns this condition.
    pub fn status(&self) -> StatusCode {
        match self {
            TokenError::Sign(_) | TokenError::Server(_) => StatusCode::INTERNAL_SERVER_ERROR,
            TokenError::InvalidClient => StatusCode::UNAUTHORIZED,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    /// RFC 6749 §5.2: the parameters go in the entity-body as
    /// `application/json`, and §5.1's `Cache-Control: no-store` /
    /// `Pragma: no-cache` bind the whole token endpoint — an error naming a
    /// client id is no more cacheable than the token a success would carry.
    /// Rendering `Display` as a bare text body left a conforming client
    /// parsing `resp.json()["error"]` with nothing to read.
    fn render(&self) -> Response {
        let body = serde_json::json!({ "error": self.to_string() });
        let mut response = body
            .to_string()
            .with_content_type("application/json")
            .with_status(self.status())
            .with_header(header::CACHE_CONTROL, "no-store")
            .with_header(header::PRAGMA, "no-cache")
            .into_response();
        // A token-endpoint refusal is not a oauth-resource refusal: the
        // caller is asking for a credential, not presenting one against a
        // resource, so pointing it at RFC 9728 discovery is misdirection. Same
        // reasoning, same marker, as the password-login path above.
        response.extensions_mut().insert(NoBearerChallenge);
        response
    }
}

/// **Deliberately not a `ResponseError`**, for the reason spelled out on
/// [`CredentialError`]'s conversion: poem overwrites a response's extensions
/// with the error's own on the way out, so the `NoBearerChallenge` marker only
/// survives when the `poem::Error` carries it. Without this, a
/// `Basic`-authenticating OAuth client was handed an RFC 9728 discovery pointer
/// instead of the reason its credentials were refused — the misdirection §5.2's
/// scheme-matching clause exists to prevent.
impl From<TokenError> for poem::Error {
    fn from(error: TokenError) -> Self {
        let mut err = poem::Error::from_response(error.render());
        err.set_data(NoBearerChallenge);
        err
    }
}
