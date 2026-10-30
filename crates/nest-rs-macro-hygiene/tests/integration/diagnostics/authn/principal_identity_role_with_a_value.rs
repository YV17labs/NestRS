//! A role is the field it is written on: it takes no value naming another.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub struct Claims {
    #[principal(actor = sub)]
    pub sub: String,
}

fn main() {}
