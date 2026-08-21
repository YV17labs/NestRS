//! The token endpoint's **successful** exchange — RFC 6749 §4.4.2's request and
//! §5.1's response.
//!
//! Held here for the reason [`TokenError`](crate::TokenError) is: §5.1 and §5.2
//! are the two halves of one response, and a conforming issuer cannot spell
//! either differently. An app that re-declared them carried a copy of the
//! specification that nothing joined back to it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// RFC 6749 §4.4.2 — the `client_credentials` access token request.
///
/// `grant_type` is a `String` rather than an enum because §5.2 obliges the
/// endpoint to answer an *unknown* grant with `unsupported_grant_type`
/// ([`TokenError::UnsupportedGrant`](crate::TokenError::UnsupportedGrant)), and
/// a typed field would turn that wire-level refusal into a deserialization
/// error the issuer never sees.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AccessTokenRequest {
    /// The grant being exercised — `client_credentials` at this endpoint.
    pub grant_type: String,
    /// §3.3 space-delimited scope list. Absent ⇒ the issuer's own default.
    #[serde(default)]
    pub scope: Option<String>,
}

/// RFC 6749 §5.1 — the successful access token response.
///
/// The three members below are the ones §5.1 marks REQUIRED for a bearer token;
/// `refresh_token` and `scope` are OPTIONAL there and are added by the issuer
/// that mints them, not by every issuer.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessTokenResponse {
    /// §5.1 REQUIRED — the token itself.
    pub access_token: String,
    /// §5.1 REQUIRED — `Bearer` for RFC 6750.
    pub token_type: String,
    /// §5.1 RECOMMENDED — lifetime in seconds, from the moment of the response.
    pub expires_in: u64,
}
