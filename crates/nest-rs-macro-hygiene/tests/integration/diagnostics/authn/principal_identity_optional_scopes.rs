//! A credential that declares its scopes answers a list, empty when it was
//! granted none: an optional list, which would answer a credential sent
//! without its `scope` claim as one scope does not apply to, is refused at the
//! field's type.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub struct Claims {
    #[principal(actor)]
    pub sub: String,
    #[principal(scopes)]
    pub scope: Option<Vec<String>>,
}

fn main() {}
