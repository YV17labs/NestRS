//! OAuth scope as a transport-agnostic dimension of a denial.
//!
//! Kept below `nest-rs-authn`, `nest-rs-authz` and `nest-rs-oauth-resource` so
//! they agree without depending on each other: a [`Scope`] is what a rule
//! requires and a protected resource advertises, [`GrantedScopes`] rides the
//! unit, [`RequiredScopes`] the response. The two markers are no decision;
//! they carry the evidence to the edge, which renders the RFC 6750
//! `insufficient_scope` challenge once for every transport.

use std::fmt;
use std::sync::Arc;

use nest_rs_core::AmbientValue;

/// An OAuth scope (RFC 6749 §3.3) the app's rules gate on, declared once as a
/// constant: a rule requires it by name
/// (`ab.can(..).requires_scope(POSTS_READ)` in `nest_rs::authz`), so a
/// scope no declaration holds does not compile, and a protected resource
/// advertises the same values (`OAuthResourceModule::for_root([..])` in
/// `nest_rs::oauth::resource`).
///
/// ```
/// use nest_rs_guards::Scope;
///
/// pub const POSTS_READ: Scope = Scope::new("posts:read");
/// pub const POSTS_WRITE: Scope = Scope::new("posts:write");
///
/// assert_eq!(POSTS_READ.as_str(), "posts:read");
/// assert_eq!(String::from(POSTS_WRITE), "posts:write");
/// ```
///
/// A token outside §3.3's grammar — empty, or holding a space, a `"` or a
/// `\` — could not ride a space-delimited `scope` nor a quoted challenge, so
/// it is refused, at compile time in a `const`:
///
/// ```compile_fail
/// use nest_rs_guards::Scope;
///
/// const SPACED: Scope = Scope::new("posts read");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Scope(&'static str);

impl Scope {
    /// The scope `token`; panics when it is not a scope token (RFC 6749
    /// §3.3: `1*( %x21 / %x23-5B / %x5D-7E )`), which in a `const` is a
    /// compile error.
    pub const fn new(token: &'static str) -> Self {
        assert!(
            is_scope_token(token),
            "a scope is a token of RFC 6749 §3.3: one or more printable ASCII characters but a \
             space, `\"` and `\\`",
        );
        Self(token)
    }

    /// The token, as a credential carries it.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl From<Scope> for String {
    fn from(scope: Scope) -> Self {
        scope.0.to_owned()
    }
}

impl PartialEq<Scope> for String {
    fn eq(&self, scope: &Scope) -> bool {
        self == scope.0
    }
}

/// RFC 6749 §3.3: `scope-token = 1*( %x21 / %x23-5B / %x5D-7E )` — printable
/// ASCII excluding space (`%x20`), `"` (`%x22`) and `\` (`%x5C`); a `const fn`,
/// so [`Scope::new`] refuses one at compile time.
pub const fn is_scope_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut at = 0;
    while at < bytes.len() {
        if !matches!(bytes[at], 0x21 | 0x23..=0x5B | 0x5D..=0x7E) {
            return false;
        }
        at += 1;
    }
    true
}

/// The scopes a caller's credential carries, attached to the unit by the
/// authentication guard and read back through
/// [`UnitContext::ambient`](nest_rs_core::UnitContext::ambient).
///
/// **Absence is not emptiness.** No `GrantedScopes` means the principal is not
/// scope-aware (a session cookie, an mTLS identity) and scope gating does not
/// apply; an empty one is an OAuth principal granted nothing, for which every
/// scoped rule is withheld.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrantedScopes(Arc<[String]>);

impl GrantedScopes {
    /// Collect the scopes a credential granted.
    pub fn new(scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(scopes.into_iter().map(Into::into).collect())
    }

    /// The granted scopes, in the order the credential listed them.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// The list itself, for a consumer that has to own it — the ability builder
    /// keeps it for the lifetime of the request. A refcount bump, not a copy.
    pub fn shared(&self) -> Arc<[String]> {
        self.0.clone()
    }
}

impl AmbientValue for GrantedScopes {}

/// The scopes an operation required but the caller's credential did not carry,
/// every one of them needed, attached to the refused response.
///
/// Read at the transport edge to build RFC 6750 §3.1's `insufficient_scope`
/// challenge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequiredScopes(Vec<String>);

impl RequiredScopes {
    /// Record the scopes the refused operation needs.
    pub fn new(scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(scopes.into_iter().map(Into::into).collect())
    }

    /// The required scopes.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Whether anything was recorded — an empty set carries no more information
    /// than a bare `403`, so the edge skips the challenge rather than emitting
    /// `scope=""`.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_is_the_token_a_credential_carries() {
        const POSTS_READ: Scope = Scope::new("posts:read");
        assert_eq!(POSTS_READ.as_str(), "posts:read");
        assert_eq!(POSTS_READ.to_string(), "posts:read");
        assert_eq!(String::from(POSTS_READ), "posts:read");
        assert!(String::from("posts:read") == POSTS_READ);
        assert!(String::from("posts:write") != POSTS_READ);
    }

    #[test]
    fn a_token_is_printable_ascii_but_a_space_a_quote_and_a_backslash() {
        for token in ["posts:read", "https://api.example.com/read", "a", "!~[]"] {
            assert!(is_scope_token(token), "{token}");
        }
        for token in [
            "",
            "posts read",
            "posts\"read",
            "posts\\read",
            "posts\tread",
            "pöst",
        ] {
            assert!(!is_scope_token(token), "{token:?}");
        }
    }

    #[test]
    #[should_panic(expected = "a scope is a token of RFC 6749 §3.3")]
    fn a_scope_outside_the_grammar_is_refused() {
        let spaced = String::from("posts read").leak();
        let _ = Scope::new(spaced);
    }

    #[test]
    fn the_grant_reaches_a_consumer_without_being_copied() {
        let granted = GrantedScopes::new(["posts:read", "posts:write"]);
        let shared = granted.shared();
        assert_eq!(shared.as_ref(), granted.as_slice());
        assert!(std::ptr::eq(shared.as_ref(), granted.as_slice()));
    }

    #[test]
    fn an_empty_grant_carries_nothing() {
        // The OAuth principal that was granted no scope — distinct from a
        // principal with no `GrantedScopes` attached at all, which is not
        // scope-aware and is never gated.
        assert!(GrantedScopes::default().as_slice().is_empty());
    }
}
