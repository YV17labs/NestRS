//! `#[principal]` names one of three roles: another word is refused listing
//! them, and so is the attribute written with none.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub struct Misnamed {
    #[principal(subject)]
    pub sub: String,
}

#[derive(Clone, PrincipalIdentity)]
pub struct Unnamed {
    #[principal]
    pub sub: String,
}

fn main() {}
