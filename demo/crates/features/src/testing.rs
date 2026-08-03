use nest_rs::authn::{JwtOptions, JwtService};
use uuid::Uuid;

use crate::{Claims, Role};

pub const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
pub const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

pub const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

/// The demo's canonical resource URI — the `aud` every minted token carries and
/// the value the `assistant` advertises in its RFC 9728 metadata. Kept in sync
/// with `NESTRS_AUTHN__AUDIENCE` in `demo/.env`: a resource server that pins an
/// audience rejects a token minted without one, so the test minter must stamp
/// the same string the deployment expects.
pub const AUDIENCE: &str = "http://localhost:3003";

pub fn token(org_id: Uuid, roles: Vec<Role>, sub: Option<Uuid>) -> String {
    token_with_scopes(org_id, roles, sub, crate::authz::constants::all())
}

/// A token delegated exactly `scopes` — the shape a third-party client (an MCP
/// client, say) holds after asking its authorization server for a subset.
///
/// The narrow case is the one worth testing: a rule gated on a scope the token
/// does not carry is withheld, so the caller is refused with an
/// `insufficient_scope` challenge naming what to go request.
pub fn token_with_scopes(
    org_id: Uuid,
    roles: Vec<Role>,
    sub: Option<Uuid>,
    scopes: Vec<String>,
) -> String {
    let mut options = JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY);
    options.audience = Some(AUDIENCE.into());
    let jwt = JwtService::new(options).expect("the dev keypair parses");
    jwt.sign(&Claims {
        sub,
        org_id,
        roles,
        scopes,
        exp: jwt.expiry(),
    })
    .expect("sign the test token")
}

pub fn token_for(org_id: &str, role: &str, sub: Option<Uuid>) -> String {
    let roles = match role {
        "admin" => vec![Role::Admin],
        _ => vec![Role::User],
    };
    token(Uuid::parse_str(org_id).expect("valid org uuid"), roles, sub)
}
