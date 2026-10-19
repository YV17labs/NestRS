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

/// Mark a `401` as no resource challenge, by inserting it into the response's
/// extensions.
///
/// A password-login rejection or a token-endpoint refusal means *these
/// credentials are wrong*, not *go discover an authorization server*; the RFC
/// 9728 interceptor (`nest-rs-oauth-resource`) reads this and leaves the
/// response alone, so it carries the challenge it was built with — the bare
/// `Bearer` the edge gives a raw-text `401` it lifts.
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
/// a quoted string could not carry, leaves the challenge without the attribute
/// and files a `warn` naming the set: the scopes are the rule's own literals.
pub(crate) fn stamp_insufficient_scope(res: &mut Response, scopes: &[String]) {
    if scopes.is_empty() {
        stamp_bearer_error(res, INSUFFICIENT_SCOPE);
        return;
    }
    if scopes.iter().all(|scope| is_scope_token(scope))
        && let Ok(value) = HeaderValue::try_from(format!(
            r#"{BEARER} error="{INSUFFICIENT_SCOPE}", scope="{}""#,
            scopes.join(" ")
        ))
    {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
        return;
    }
    tracing::warn!(
        target: crate::target::HTTP,
        ?scopes,
        "a required scope is not a scope-token; the challenge is sent without its scope attribute",
    );
    stamp_bearer_error(res, INSUFFICIENT_SCOPE);
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
    use nest_rs_testing::LogCapture;

    use super::*;

    const LEFT_OUT: &str =
        "a required scope is not a scope-token; the challenge is sent without its scope attribute";

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
        let logs = LogCapture::install();
        assert_eq!(
            challenge(&["posts:read", "audit"]),
            r#"Bearer error="insufficient_scope", scope="posts:read audit""#,
        );
        logs.expect_none(crate::target::HTTP, LEFT_OUT);
    }

    /// A quote would end the attribute early; a space would split one scope in
    /// two: such a set is not said at all, and the operator is told why.
    #[test]
    fn a_scope_outside_the_grammar_leaves_the_attribute_out() {
        for refused in [&["posts\"read"][..], &["posts read"], &[""]] {
            let logs = LogCapture::install();
            assert_eq!(
                challenge(refused),
                r#"Bearer error="insufficient_scope""#,
                "{refused:?}"
            );
            let filed = logs.expect_one(crate::target::HTTP, LEFT_OUT);
            assert_eq!(filed.level, "warn");
            assert_eq!(filed.field("scopes"), Some(format!("{refused:?}")));
        }
    }

    /// No scope to name is no malformed one: the bare challenge, filed nowhere.
    #[test]
    fn an_empty_set_answers_the_bare_challenge_and_files_nothing() {
        let logs = LogCapture::install();
        assert_eq!(challenge(&[]), r#"Bearer error="insufficient_scope""#);
        logs.expect_none(crate::target::HTTP, LEFT_OUT);
    }
}
