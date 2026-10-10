//! The `WWW-Authenticate: Bearer` challenge grammar — RFC 6750 §3.

use poem::Response;
use poem::http::{HeaderValue, header};

pub use crate::problem::BEARER;

/// RFC 6750 §3.1's `invalid_request` — the request is malformed as a
/// credential-bearing request (a repeated parameter, more than one method used
/// to transmit the token).
pub const INVALID_REQUEST: &str = "invalid_request";

/// RFC 6750 §3.1's `invalid_token` — a credential arrived and was refused
/// (expired, malformed, revoked, or wrong signature).
pub const INVALID_TOKEN: &str = "invalid_token";

/// RFC 6750 §3.1's `insufficient_scope` — the credential verified but does not
/// carry the scope the operation requires.
pub const INSUFFICIENT_SCOPE: &str = "insufficient_scope";

/// Render `Bearer error="<code>"` — the minimal conformant challenge.
///
/// `code` is always a constant from §3.1's closed set, never caller data, so
/// nothing in it needs escaping.
fn bearer_error(code: &str) -> String {
    format!(r#"{BEARER} error="{code}""#)
}

/// Render `Bearer error="<code>", error_description="<why>"` — §3's optional
/// second parameter, for the sites that hold a client-safe reason.
///
/// `why` is quoted verbatim, so it must be a message the framework composed:
/// a `"` in it would end the parameter early.
pub fn bearer_error_described(code: &str, why: &str) -> String {
    format!(r#"{BEARER} error="{code}", error_description="{why}""#)
}

/// The same value as a [`HeaderValue`], or `None` when it could not be encoded.
fn bearer_error_value(code: &str) -> Option<HeaderValue> {
    bearer_error(code).parse().ok()
}

/// Write `WWW-Authenticate: Bearer error="<code>"` onto a refusal, replacing
/// whatever stood there; a code that cannot be encoded leaves the response
/// untouched.
pub fn stamp_bearer_error(res: &mut Response, code: &str) {
    if let Some(value) = bearer_error_value(code) {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
}
