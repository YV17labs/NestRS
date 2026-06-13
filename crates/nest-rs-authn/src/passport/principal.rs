//! [`PrincipalIdentity`] — the audit identity every principal exposes.
//!
//! Security events (denials, auth failures) must be answerable under
//! incident: *which actor was denied what?* The framework records
//! `actor_id` on the request span the moment authentication succeeds
//! ([`AuthGuard`](crate::AuthGuard)), so every downstream event — a
//! row-level denial in the ORM, a masked response, a guard rejection —
//! inherits the identity without each call site threading it.

/// Stable audit identifier of a principal — the value recorded as the
/// request span's `actor_id` field. Return `None` when the principal
/// carries no stable identity (an anonymous or machine principal without
/// a subject).
pub trait PrincipalIdentity {
    fn actor_id(&self) -> Option<String>;
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
