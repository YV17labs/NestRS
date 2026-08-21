//! Integration tests for `nest-rs-authn`. Layout strictly mirrors `src/` (see CLAUDE.md).
//!
//! - This file is the only `tests/*.rs` binary; paths under `tests/` are modules.
//! - Shared fixtures live below at the suite root (`crate::…`), so every module
//!   in the tree mirrors a `src/` counterpart.

mod config;
mod credentials;
mod error;
mod guard;
mod module;
mod password;
mod service;
mod strategies;

use poem::http::header;
use poem::{Body, Request};

/// Ed25519 key pair used across nestrs dev and e2e apps.
pub const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
pub const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

pub fn request(headers: &[(&str, &str)]) -> Request {
    let mut req = Request::builder().body(Body::empty());
    for (name, value) in headers {
        req.headers_mut().insert(
            header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            header::HeaderValue::from_str(value).expect("header value"),
        );
    }
    req
}
