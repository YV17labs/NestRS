//! OAuth scope as a transport-agnostic dimension of a denial.
//!
//! Two markers travel opposite directions, and keeping them here — below both
//! `nest-rs-authn` (which mints one) and `nest-rs-authz` (which reads it) —
//! is what lets the two crates agree without depending on each other:
//!
//! * [`GrantedScopes`] rides the **request**: what the caller's credential
//!   actually carries, attached by the authn guard the moment a principal is
//!   established.
//! * [`RequiredScopes`] rides the **response**: what the refused operation
//!   would have needed, attached wherever a [`Denial::InsufficientScope`]
//!   becomes a transport response.
//!
//! Neither is a decision. The decision stays in the guard — these carry the
//! *evidence* to the edge, so one interceptor can turn it into the RFC 6750
//! `insufficient_scope` challenge for every transport at once instead of each
//! denial site hand-writing a header.
//!
//! [`Denial::InsufficientScope`]: crate::Denial::InsufficientScope

use std::sync::Arc;

/// The scopes a caller's credential carries, attached to the request by the
/// authentication guard.
///
/// **Absence is not emptiness.** No `GrantedScopes` at all means the principal
/// is not scope-aware — a session cookie, an mTLS identity, a test fixture —
/// and scope gating does not apply to it: a rule that
/// [`requires_scope`](https://nestrs.dev/security/authorization/scopes/) still
/// materializes. An *empty* `GrantedScopes` is the opposite statement: an OAuth
/// principal that was granted nothing, for which every scoped rule is withheld.
/// Conflating the two would either tax every non-OAuth app or silently disarm
/// the check for OAuth ones.
///
/// Held behind an `Arc` because the list is built once — when the authn guard
/// publishes it — and read from there by every layer downstream. A `Vec` made
/// the authorization guard deep-copy it a second time on every authenticated
/// request to seed the ability builder; [`shared`](Self::shared) hands over the
/// same allocation instead.
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
/// Read at the transport edge to build the `WWW-Authenticate:
/// Bearer error="insufficient_scope", scope="…"` challenge RFC 6750 §3.1
/// prescribes — the header that tells an OAuth client *which* narrower token it
/// holds and what to ask the authorization server for instead.
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
        // The authz guard seeds the ability builder from this on every
        // authenticated request; handing over the allocation is the difference
        // between a refcount bump and a `String` per scope per request.
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
/// A third marker, riding the **response** like [`RequiredScopes`] and living
/// here for the same reason: its writers and its reader sit in different
/// crates. A password-login rejection and a token-endpoint refusal (both
/// `nest-rs-authn`) mean *these credentials are wrong*,
/// not *go discover an authorization server*; the interceptor that would
/// otherwise stamp the RFC 9728 pointer (`nest-rs-oauth-resource`) reads this
/// and leaves the response alone. `POST /auth/login` and `POST /token` are not
/// protected resources — pointing those callers at discovery is misdirection.
///
/// An app whose own `401` means the same thing marks it the same way.
#[derive(Clone, Copy, Debug)]
pub struct NoBearerChallenge;
