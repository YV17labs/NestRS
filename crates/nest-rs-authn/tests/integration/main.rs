//! Integration tests for `nest-rs-authn`, mirroring `src/`; shared fixtures sit
//! at the suite root.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod algorithm;
mod config;
mod credentials;
mod development;
mod error;
mod guard;
mod jwks;
mod module;
mod password;
mod principal_identity;
mod service;
mod strategies;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, Header};
use nest_rs_authn::JwtOptions;
use nest_rs_http::{HeaderName, HeaderValue, Request};

/// The issuer and the audience every fixture service names: `JwtService::new`
/// requires both (RFC 9068 §2.2, §4).
pub const ISSUER: &str = "https://auth.example";
pub const AUDIENCE: &str = "https://api.example";

/// `options` naming [`ISSUER`] and [`AUDIENCE`].
pub fn named(mut options: JwtOptions) -> JwtOptions {
    options.issuer = Some(ISSUER.into());
    options.audience = Some(AUDIENCE.into());
    options
}

/// The header a conformant issuer stamps (RFC 9068 §2.1). Fixtures mint
/// tokens by hand to exercise *other* checks; without this they would all be
/// refused by the `typ` check instead, which is not what any of them asserts.
pub fn at_jwt_header(alg: Algorithm) -> Header {
    let mut header = Header::new(alg);
    header.typ = Some("at+jwt".into());
    header
}

/// The claims `token` carries, read without verifying it.
pub fn payload(token: &str) -> serde_json::Value {
    let encoded = token.split('.').nth(1).expect("a compact JWS");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).expect("base64url"))
        .expect("a JSON payload")
}

/// Ed25519 key pair used across nestrs dev and e2e apps.
pub const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
pub const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

pub fn request(headers: &[(&str, &str)]) -> Request {
    let mut req = Request::builder().finish();
    for (name, value) in headers {
        req.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            HeaderValue::from_str(value).expect("header value"),
        );
    }
    req
}
