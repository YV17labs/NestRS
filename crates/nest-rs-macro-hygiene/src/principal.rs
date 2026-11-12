//! `#[derive(PrincipalIdentity)]` — its `PrincipalIdentity` and `AmbientValue`
//! impls resolve through the umbrella alone, each role read off its field:
//! an optional tenant, a scope list.

use nest_rs::authn::PrincipalIdentity;

/// The claims a hygiene token carries.
#[derive(Clone, PrincipalIdentity)]
pub struct HygieneClaims {
    /// The subject the token names.
    #[principal(actor)]
    pub sub: String,
    /// The tenant it acts within, when it is bound to one.
    #[principal(tenant)]
    pub org_id: Option<String>,
    /// The scopes it was granted.
    #[principal(scopes)]
    pub scope: Vec<String>,
}
