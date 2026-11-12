//! A principal is the subject its credential names: claims that mark a tenant
//! and scopes but no subject are refused, naming the field attribute that marks
//! it.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub struct Claims {
    #[principal(tenant)]
    pub org_id: String,
    #[principal(scopes)]
    pub scopes: Vec<String>,
}

fn main() {}
