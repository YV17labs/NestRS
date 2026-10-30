//! A subject is read as the text it prints: a field whose type prints nothing,
//! or an optional one whose value prints nothing, is refused at its type.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone)]
pub struct Opaque;

#[derive(Clone, PrincipalIdentity)]
pub struct Claims {
    #[principal(actor)]
    pub sub: Vec<u8>,
    #[principal(tenant)]
    pub org: Option<Opaque>,
}

fn main() {}
