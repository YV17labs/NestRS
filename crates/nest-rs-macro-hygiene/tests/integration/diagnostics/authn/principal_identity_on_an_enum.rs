//! The derive reads the fields of one claims struct: an enum is refused, and
//! so is `#[principal]` written on the struct rather than on a field.

use nest_rs::authn::PrincipalIdentity;

#[derive(Clone, PrincipalIdentity)]
pub enum Caller {
    User(String),
    Client(String),
}

#[derive(Clone, PrincipalIdentity)]
#[principal(actor)]
pub struct Claims {
    pub sub: String,
}

fn main() {}
