//! [`PrincipalIdentity`] — what every principal says about itself: its audit
//! identity, the tenant it acts within, and the scopes its credential carries.
//!
//!
//! The framework records `actor_id` on the request span when authentication
//! succeeds, so every downstream event inherits it, and makes the tenant
//! ambient for the request and every job it pushes.

/// Stable audit identifier of a principal — the value recorded as the
/// request span's `actor_id` field, and the subject an MCP session is bound
/// to. Return the subject the verified credential names: an access token's
/// `sub`, which RFC 9068 §2.2 requires, the client's own id when the client
/// acts on its own behalf.
pub trait PrincipalIdentity {
    /// The subject the verified credential names, or `None` for a principal
    /// naming none, which a gated MCP endpoint opens no session for.
    fn actor_id(&self) -> Option<String>;

    /// The tenant this principal acts within, or `None` (the default) for one
    /// bound to no tenant.
    ///
    /// Unlike [`actor_id`](Self::actor_id), an authorization input: the
    /// guard makes it ambient for the request and every continuation of it,
    /// and every job the request pushes runs for it —
    /// [`current_tenant_id`](nest_rs_core::current_tenant_id) reads it back. Read it off the
    /// verified credential alone, a claim its issuer signed, never off what
    /// the caller sends beside it. An empty id names no tenant.
    fn tenant_id(&self) -> Option<String> {
        None
    }

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
