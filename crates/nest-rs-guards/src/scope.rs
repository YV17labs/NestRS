//! OAuth scope as a transport-agnostic dimension of a denial.
//!
//! Two markers, kept below both `nest-rs-authn` and `nest-rs-authz` so the two
//! agree without depending on each other: [`GrantedScopes`] rides the request,
//! [`RequiredScopes`] the response. Neither is a decision; they carry the
//! evidence to the edge, which renders the RFC 6750 `insufficient_scope`
//! challenge once for every transport.

use std::sync::Arc;

/// The scopes a caller's credential carries, attached to the request by the
/// authentication guard.
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

/// The scopes an operation required but the caller's credential did not carry,
/// attached to the refused response.
///
/// Read at the transport edge to build RFC 6750 §3.1's `insufficient_scope`
/// challenge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequiredScopes(Vec<String>);

impl RequiredScopes {
    /// Record the scopes that would have granted the refused operation.
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
    fn the_grant_reaches_a_consumer_without_being_copied() {
        let granted = GrantedScopes::new(["posts:read", "posts:write"]);
        let shared = granted.shared();
        assert_eq!(shared.as_ref(), granted.as_slice());
        assert!(std::ptr::eq(shared.as_ref(), granted.as_slice()));
    }

    #[test]
    fn an_empty_grant_carries_nothing() {
        // The OAuth principal that was granted no scope — distinct from a
        // principal with no `GrantedScopes` extension at all, which is not
        // scope-aware and is never gated.
        assert!(GrantedScopes::default().as_slice().is_empty());
    }
}

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
