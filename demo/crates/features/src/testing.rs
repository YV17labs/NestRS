//! Dev-only test support shared by the app e2e suites (feature
//! `test-support`, enabled from `dev-dependencies` — nothing here ships in a
//! production build).
//!
//! One home for the well-known dev keypair, the seeded org id, and
//! [`Claims`] token minting, so rotating the key or reshaping `Claims` is a
//! single edit instead of one per suite.

use nest_rs_authn::{JwtOptions, JwtService};
use uuid::Uuid;

use crate::{Claims, Role};

/// The dev EdDSA signing key every e2e suite mints tokens with. Matches
/// [`DEV_PUBLIC_KEY`]; never a production key.
pub const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
/// The matching verification key the suites hand to `JwtConfig`.
pub const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

/// The seeded default org's id, as the suites' string literal.
pub const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

/// Mint a dev-signed [`Claims`] token.
pub fn token(org_id: Uuid, roles: Vec<Role>, sub: Option<Uuid>) -> String {
    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    jwt.sign(&Claims {
        sub,
        org_id,
        roles,
        exp: jwt.expiry(),
    })
    .expect("sign the test token")
}

/// [`token`] over a string org id and the suites' `"admin"`/`"user"` role
/// shorthand.
pub fn token_for(org_id: &str, role: &str, sub: Option<Uuid>) -> String {
    let roles = match role {
        "admin" => vec![Role::Admin],
        _ => vec![Role::User],
    };
    token(Uuid::parse_str(org_id).expect("valid org uuid"), roles, sub)
}
