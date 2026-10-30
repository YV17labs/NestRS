//! A strategy authenticates one concrete principal, and an ambient value
//! outlives the request it was read from: claims generic over a type or a
//! lifetime are refused.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub struct Claims<T> {
    #[principal(actor)]
    pub sub: String,
    pub extra: T,
}

#[derive(Clone, PrincipalIdentity)]
pub struct Borrowed<'a> {
    #[principal(actor)]
    pub sub: &'a str,
}

fn main() {}
