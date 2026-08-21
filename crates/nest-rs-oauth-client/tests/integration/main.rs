//! Integration suite for `nest-rs-oauth-client` — the crate's public API in
//! process, no DB and no network. Paths mirror `src/`.

mod client;
mod config;
mod module;

use nest_rs_authn::{JwtOptions, JwtService};

/// 32 bytes — HS256's floor, enforced in `JwtService::new`. Shared: the
/// transaction a client mints is verified by the same service in two modules,
/// and a second literal is a second thing to keep at the floor.
pub const SECRET: &str = "test-secret-padded-to-thirty-two-b";

/// The signer behind every handshake transaction this suite mints.
pub fn jwt() -> JwtService {
    JwtService::new(JwtOptions::new(SECRET)).expect("HMAC JwtService")
}
