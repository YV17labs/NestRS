//! The `WWW-Authenticate: Bearer` challenge grammar — RFC 6750 §3.
//!
//! Declared here because this is the crate every writer of that header already
//! depends on: `nest-rs-guards` renders a denial, `nest-rs-authn` renders a
//! rejected credential, and `nest-rs-oauth-resource` splices the RFC 9728
//! pointer onto whatever the first two wrote. Each holds a different subset of
//! the facts, so they cannot be merged into one caller — but the *grammar* is
//! one production, and three hand-written `format!`s of it had already drifted
//! into three different parameter sets for one failure.

use poem::Response;
use poem::http::{HeaderValue, header};

/// The scheme name, matched case-insensitively per RFC 7235 §2.1.
pub const BEARER: &str = "Bearer";

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
/// RFC 6750 §3 requires the scheme "be followed by one or more auth-param
/// values", and §3.1 defines the `error` codes. `code` is always a framework
/// constant from the specification's closed set, never caller data, so the
/// result cannot carry a character that would need escaping.
fn bearer_error(code: &str) -> String {
    format!(r#"{BEARER} error="{code}""#)
}

/// Render `Bearer error="<code>", error_description="<why>"` — §3's optional
/// second parameter, for the sites that hold a client-safe reason.
///
/// `why` is quoted verbatim, so it must be a message the framework composed:
/// §3's `quoted-string` has no escape for a `"` and one would end the parameter
/// early. Every caller passes an opaque constant for that reason.
pub fn bearer_error_described(code: &str, why: &str) -> String {
    format!(r#"{BEARER} error="{code}", error_description="{why}""#)
}

/// The same value as a [`HeaderValue`], or `None` when it could not be encoded.
///
/// Returns rather than panics: a header that cannot be built is not built, and
/// the caller decides whether that silence is worth an event.
fn bearer_error_value(code: &str) -> Option<HeaderValue> {
    bearer_error(code).parse().ok()
}

/// Write `WWW-Authenticate: Bearer error="<code>"` onto a refusal, replacing
/// whatever stood there.
///
/// One writer for the two sites that hold a §3.1 code and a `Response`: the
/// guard-denial renderer, and the deferred `401` a `#[public]` route answers
/// when the principal it needs was refused upstream. A code that cannot be
/// encoded leaves the response untouched rather than panicking — the caller
/// decides whether that silence is worth an event, and neither of the two
/// passes anything but a constant from the set above.
pub fn stamp_bearer_error(res: &mut Response, code: &str) {
    if let Some(value) = bearer_error_value(code) {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
}
