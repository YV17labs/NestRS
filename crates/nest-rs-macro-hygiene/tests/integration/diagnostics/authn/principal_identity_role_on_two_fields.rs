//! A principal names one subject, acts within one tenant and carries one scope
//! list: a role marked twice — on two fields, or twice in one list — is
//! refused at the second, and so is a second `#[principal]` on one field.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub struct TwoSubjects {
    #[principal(actor)]
    pub sub: String,
    #[principal(actor)]
    pub client_id: String,
}

#[derive(Clone, PrincipalIdentity)]
pub struct TenantTwice {
    #[principal(actor, tenant, tenant)]
    pub sub: String,
}

#[derive(Clone, PrincipalIdentity)]
pub struct WrittenTwice {
    #[principal(actor)]
    #[principal(tenant)]
    pub sub: String,
}

fn main() {}
