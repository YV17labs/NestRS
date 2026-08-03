//! [`PrincipalIdentity`] — the audit identity every principal exposes.
//!
//! Security events (denials, auth failures) must be answerable under
//! incident: *which actor was denied what?* The framework records
//! `actor_id` on the request span the moment authentication succeeds
//! ([`AuthnGuard`](crate::AuthnGuard)), so every downstream event — a
//! row-level denial in the ORM, a masked response, a guard rejection —
//! inherits the identity without each call site threading it.

/// Stable audit identifier of a principal — the value recorded as the
/// request span's `actor_id` field. Return `None` when the principal
/// carries no stable identity (an anonymous or machine principal without
/// a subject).
pub trait PrincipalIdentity {
    /// The principal's stable audit id, or `None` for an anonymous/machine
    /// principal with no subject.
    fn actor_id(&self) -> Option<String>;

    /// The OAuth scopes this credential was granted, or `None` when the
    /// principal is not scope-aware.
    ///
    /// **The two answers mean different things, and the default is the safe
    /// one.** `None` — the default, and what a session cookie, an mTLS identity
    /// or a test fixture returns — says scope is not a dimension of this
    /// credential, so authorization rules gated on a scope still apply in full.
    /// `Some(&[])` says the opposite: an OAuth credential that was granted
    /// nothing, for which every scoped rule is withheld.
    ///
    /// Implement it on a resource server's claims type, reading the `scope`
    /// claim (RFC 6749 §3.3: space-delimited) or `scp`, and the framework does
    /// the rest — the authn guard publishes the result as
    /// [`GrantedScopes`](nest_rs_guards::GrantedScopes), the ability layer
    /// withholds the rules the caller cannot reach, and the refusal reaches the
    /// client as an `insufficient_scope` challenge naming what to ask for.
    ///
    /// It is **not** an authorization decision, and cannot be used as one: it
    /// reports what the credential carries. What that permits is decided in the
    /// ability rules, which is a guard.
    fn scopes(&self) -> Option<&[String]> {
        None
    }
}

/// The anonymous principal: no identity.
impl PrincipalIdentity for () {
    fn actor_id(&self) -> Option<String> {
        None
    }
}

/// Test/fixture principals.
impl PrincipalIdentity for &'static str {
    fn actor_id(&self) -> Option<String> {
        Some((*self).to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_principal_has_no_actor_id() {
        assert_eq!(().actor_id(), None);
    }

    #[test]
    fn str_principal_is_its_own_actor_id() {
        assert_eq!("ada".actor_id(), Some("ada".to_owned()));
    }
}
