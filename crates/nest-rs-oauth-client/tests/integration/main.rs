//! Integration suite for `nest-rs-oauth-client` — the crate's public API in
//! process, no DB and no network. Paths mirror `src/`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod client;
mod config;
mod module;
mod redirect;

use nest_rs_authn::{JwtOptions, JwtService};

/// 32 bytes, HS256's floor enforced in `JwtService::new`.
pub const SECRET: &str = "test-secret-padded-to-thirty-two-b";

/// The issuer and the audience of the app this suite's client signs in for,
/// which a `JwtService` requires.
pub const APP: &str = "https://app.example";

/// `options` naming [`APP`] as their issuer and audience.
pub fn named(mut options: JwtOptions) -> JwtOptions {
    options.issuer = Some(APP.into());
    options.audience = Some(APP.into());
    options
}

/// The signer behind every handshake transaction this suite mints.
pub fn jwt() -> JwtService {
    JwtService::new(named(JwtOptions::new(SECRET))).expect("HMAC JwtService")
}
