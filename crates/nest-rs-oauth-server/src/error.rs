//! RFC 6749 §5.2's token-endpoint error vocabulary, and the client
//! authentication (§2.3.1) an issuer performs before it mints anything.

use nest_rs_http::challenge::NoBearerChallenge;
use nest_rs_http::{HeaderValue, Response, ResponseError, StatusCode, header};

/// The challenge a client refused over HTTP Basic is answered with (RFC 7617
/// §2): the scheme it used, and the protection space every endpoint taking
/// these client credentials shares.
const BASIC_CHALLENGE: &str = r#"Basic realm="authorization server""#;

/// How a client tried to authenticate at the token endpoint — RFC 7591 §2's
/// `token_endpoint_auth_method` values, which decide how its refusal answers
/// ([`TokenError::InvalidClient`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenEndpointAuthMethod {
    /// `none`: the request carried no client authentication.
    None,
    /// `client_secret_post`: the client's credentials in the request body
    /// (RFC 6749 §2.3.1).
    ClientSecretPost,
    /// `client_secret_basic`: HTTP Basic in the `Authorization` header (RFC
    /// 6749 §2.3.1) — the method a request whose `Authorization` header named
    /// any scheme attempted, since Basic is the one this server reads there.
    ClientSecretBasic,
}

/// Token-endpoint failure — RFC 6749 §5.2's closed set, all six of it.
///
/// `Display` yields the wire code of the JSON response's `error` member. The
/// response is marked [`NoBearerChallenge`]: it is no resource refusal, and
/// the one challenge it carries is a refused client's own (§5.2).
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
    /// unsupported authentication method. A client that authenticated through
    /// the `Authorization` header is answered `401` with the `Basic` challenge
    /// RFC 6749 §5.2 requires; one that did not, `400`, §5.2's own status, so
    /// no browser carrying a public client raises a credentials prompt.
    #[error("invalid_client")]
    InvalidClient(TokenEndpointAuthMethod),
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
            TokenError::InvalidClient(TokenEndpointAuthMethod::ClientSecretBasic) => {
                StatusCode::UNAUTHORIZED
            }
            _ => StatusCode::BAD_REQUEST,
        }
    }

    /// The operator's line for the cause an opaque `server_error` withholds:
    /// its whole chain, at `error`, or at `warn` behind a `503`.
    fn file(&self) {
        match self {
            TokenError::Sign(_) => tracing::error!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(self),
                "token request failed",
            ),
            TokenError::Server(_) => tracing::warn!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(self),
                "token request unavailable",
            ),
            TokenError::InvalidRequest
            | TokenError::UnsupportedGrant
            | TokenError::InvalidGrant
            | TokenError::UnauthorizedClient
            | TokenError::InvalidScope
            | TokenError::InvalidClient(_) => {}
        }
    }
}

/// RFC 6749 §5.2's JSON body, with §5.1's `no-store` and `no-cache`, which
/// bind the whole token endpoint; `?` on one in a handler answers it, and files
/// the cause a `server_error` withholds.
impl ResponseError for TokenError {
    fn status(&self) -> StatusCode {
        TokenError::status(self)
    }

    fn as_response(&self) -> Response {
        self.file();
        let body = serde_json::json!({ "error": self.to_string() });
        let status = TokenError::status(self);
        let mut response = Response::builder()
            .status(status)
            .content_type(HeaderValue::from_static("application/json"))
            .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .header(header::PRAGMA, HeaderValue::from_static("no-cache"))
            // A token-endpoint refusal is not a resource refusal: no RFC 9728 pointer.
            .extension(NoBearerChallenge);
        if status == StatusCode::UNAUTHORIZED {
            response = response.header(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(BASIC_CHALLENGE),
            );
        }
        response.body(body.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 9110 §15.5.2: a `401` carries a challenge, whichever refusal it is.
    #[test]
    fn no_refusal_answers_a_401_without_a_challenge() {
        let refusals = [
            TokenError::InvalidRequest,
            TokenError::UnsupportedGrant,
            TokenError::InvalidGrant,
            TokenError::UnauthorizedClient,
            TokenError::InvalidScope,
            TokenError::InvalidClient(TokenEndpointAuthMethod::None),
            TokenError::InvalidClient(TokenEndpointAuthMethod::ClientSecretPost),
            TokenError::InvalidClient(TokenEndpointAuthMethod::ClientSecretBasic),
            TokenError::Sign(anyhow::anyhow!("signing key unusable")),
            TokenError::Server(anyhow::anyhow!("identity store unreachable")),
        ];
        let mut unauthorized = 0;
        for refusal in refusals {
            let response = refusal.as_response();
            let challenge = response.headers().get(header::WWW_AUTHENTICATE);
            if response.status() == StatusCode::UNAUTHORIZED {
                unauthorized += 1;
                assert_eq!(
                    challenge.and_then(|value| value.to_str().ok()),
                    Some(BASIC_CHALLENGE),
                    "{refusal:?}"
                );
            } else {
                assert!(challenge.is_none(), "{refusal:?}");
            }
        }
        assert_eq!(unauthorized, 1, "Basic is the one 401");
    }
}
