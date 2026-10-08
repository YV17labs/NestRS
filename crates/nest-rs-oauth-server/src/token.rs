//! The token endpoint's **successful** exchange — RFC 6749 §4.4.2's request and
//! §5.1's response.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// RFC 6749 §4.4.2 — the `client_credentials` access token request.
///
/// `grant_type` is a `String`: §5.2 answers an unknown grant with
/// [`TokenError::UnsupportedGrant`](crate::TokenError::UnsupportedGrant), which an
/// enum would turn into a deserialization error the issuer never sees.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AccessTokenRequest {
    /// The grant being exercised — `client_credentials` at this endpoint.
    pub grant_type: String,
    /// §3.3 space-delimited scope list. Absent ⇒ the issuer's own default.
    #[serde(default)]
    pub scope: Option<String>,
}

/// RFC 6749 §5.1 — the successful access token response.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessTokenResponse {
    /// §5.1 REQUIRED — the token itself.
    pub access_token: String,
    /// §5.1 REQUIRED — `Bearer` for RFC 6750.
    pub token_type: String,
    /// §5.1 RECOMMENDED — lifetime in seconds, from the moment of the response.
    pub expires_in: u64,
}
