//! RFC 6749 §5.2's token-endpoint error vocabulary, and the client
//! authentication (§2.3.1) an issuer performs before it mints anything.

use nest_rs_http::challenge::NoBearerChallenge;
use nest_rs_http::{HeaderValue, Response, ResponseError, StatusCode, header};

/// Token-endpoint failure — RFC 6749 §5.2's closed set, all six of it.
///
/// `Display` yields the wire code of the JSON response's `error` member. §5.2's
/// `WWW-Authenticate` matching the client's scheme is not sent, as the scheme is
/// not carried; the response is marked [`NoBearerChallenge`] instead.
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
    /// client (400) — and the code a rejected resource-owner password takes
    /// (§5.2), never [`InvalidClient`](Self::InvalidClient).
    #[error("invalid_grant")]
    InvalidGrant,
    /// This client authenticated, but its registration does not permit the
    /// requested grant type (400).
    #[error("unauthorized_client")]
    UnauthorizedClient,
    /// The requested scope is unknown or not permitted for this client (400).
    #[error("invalid_scope")]
    InvalidScope,
    /// Client authentication failed — unknown client, none included, or an
    /// unsupported authentication method (401).
    #[error("invalid_client")]
    InvalidClient,
    /// Internal signing failure. `Display` is the opaque RFC 6749
    /// `server_error`; the source stays attached for `tracing`.
    #[error("server_error")]
    Sign(#[source] anyhow::Error),
    /// A backend dependency (e.g. the identity store) was unreachable while
    /// resolving the grant (503, RFC 9110 §15.6.4). `Display` is the opaque
    /// `server_error`; the source stays attached for `tracing`.
    #[error("server_error")]
    Server(#[source] anyhow::Error),
}

impl TokenError {
    /// The HTTP status RFC 6749 §5.2 assigns this condition — and, for the two
    /// failures §5.2 leaves to HTTP, the one RFC 9110 does: `500` for this
    /// server's own, `503` for a dependency that did not answer.
    pub fn status(&self) -> StatusCode {
        match self {
            TokenError::Sign(_) => StatusCode::INTERNAL_SERVER_ERROR,
            TokenError::Server(_) => StatusCode::SERVICE_UNAVAILABLE,
            TokenError::InvalidClient => StatusCode::UNAUTHORIZED,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

/// RFC 6749 §5.2's JSON body, with §5.1's `no-store` and `no-cache`, which
/// bind the whole token endpoint; `?` on one in a handler answers it.
impl ResponseError for TokenError {
    fn status(&self) -> StatusCode {
        TokenError::status(self)
    }

    fn as_response(&self) -> Response {
        let body = serde_json::json!({ "error": self.to_string() });
        Response::builder()
            .status(TokenError::status(self))
            .content_type(HeaderValue::from_static("application/json"))
            .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .header(header::PRAGMA, HeaderValue::from_static("no-cache"))
            // A token-endpoint refusal is not a resource refusal: no RFC 9728 pointer.
            .extension(NoBearerChallenge)
            .body(body.to_string())
    }
}
