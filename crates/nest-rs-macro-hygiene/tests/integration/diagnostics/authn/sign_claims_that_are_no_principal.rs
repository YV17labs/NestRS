//! An access token names its subject, so a signer takes a principal: claims
//! naming no one are refused at their type, whatever they serialize to.

use nest_rs::authn::JwtService;

#[derive(serde::Serialize)]
pub struct Claims {
    pub roles: Vec<String>,
}

pub fn mint(jwt: &JwtService, claims: &Claims) {
    let _ = jwt.sign(claims);
}

fn main() {}
