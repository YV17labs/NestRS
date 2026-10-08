//! [`PrincipalIdentity`] — the audit identity every principal exposes.
//!
//!
//! The framework records `actor_id` on the request span when authentication
//! succeeds, so every downstream event inherits it.

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
    ///
    /// `None` (the default: a session, an mTLS identity) means scope is not a
    /// dimension of this credential, so scoped rules apply in full; `Some(&[])`
    /// is an OAuth credential granted nothing, for which every scoped rule is
    /// withheld.
    ///
    /// Implement it on a resource server's claims type, reading `scope` (RFC
    /// 6749 §3.3, space-delimited) or `scp`. It reports what the credential
    /// carries and is not an authorization decision.
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
