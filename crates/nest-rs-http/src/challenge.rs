//! The `WWW-Authenticate: Bearer` challenge grammar — RFC 6750 §3.

use http::{HeaderValue, header};

use crate::response::Response;

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

/// Opt a `401` out of the `Bearer` challenge, by inserting it into the
/// response's extensions.
///
/// A password-login rejection or a token-endpoint refusal means *these
/// credentials are wrong*, not *go discover an authorization server*; the RFC
/// 9728 interceptor (`nest-rs-oauth-resource`) reads this and leaves the
/// response alone.
///
/// An app whose own `401` means the same thing marks it the same way.
#[derive(Clone, Copy, Debug)]
pub struct NoBearerChallenge;

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
    stamp_bearer_error_on(res.headers_mut(), code);
}

/// Write `WWW-Authenticate: Bearer error="insufficient_scope", scope="<…>"`
/// onto a refusal: RFC 6750 §3's `scope`, the scopes the request needs, every
/// one of them, space-delimited. A scope outside RFC 6749 §3.3's grammar, which
/// a quoted string could not carry, leaves the challenge without the attribute.
pub(crate) fn stamp_insufficient_scope(res: &mut Response, scopes: &[String]) {
    let well_formed = !scopes.is_empty() && scopes.iter().all(|scope| is_scope_token(scope));
    if well_formed
        && let Ok(value) = HeaderValue::try_from(format!(
            r#"{BEARER} error="{INSUFFICIENT_SCOPE}", scope="{}""#,
            scopes.join(" ")
        ))
    {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    } else {
        stamp_bearer_error(res, INSUFFICIENT_SCOPE);
    }
}

/// RFC 6749 §3.3's `scope-token`: one or more `NQCHAR`.
fn is_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| matches!(b, 0x21 | 0x23..=0x5B | 0x5D..=0x7E))
}

/// [`stamp_bearer_error`] on a head's headers, whichever response holds them.
pub(crate) fn stamp_bearer_error_on(headers: &mut http::HeaderMap, code: &str) {
    if let Some(value) = bearer_error_value(code) {
        headers.insert(header::WWW_AUTHENTICATE, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(scopes: &[&str]) -> String {
        let mut res = Response::builder().finish();
        let scopes: Vec<String> = scopes.iter().map(|&scope| scope.to_owned()).collect();
        stamp_insufficient_scope(&mut res, &scopes);
        res.headers()[header::WWW_AUTHENTICATE]
            .to_str()
            .expect("ASCII")
            .to_owned()
    }

    #[test]
    fn the_scope_attribute_lists_every_scope_needed() {
        assert_eq!(
            challenge(&["posts:read", "audit"]),
            r#"Bearer error="insufficient_scope", scope="posts:read audit""#,
        );
    }

    /// A quote would end the attribute early; a space would split one scope in
    /// two: such a set is not said at all.
    #[test]
    fn a_scope_outside_the_grammar_leaves_the_attribute_out() {
        for refused in [&["posts\"read"][..], &["posts read"], &[""], &[]] {
            assert_eq!(
                challenge(refused),
                r#"Bearer error="insufficient_scope""#,
                "{refused:?}"
            );
        }
    }
}
