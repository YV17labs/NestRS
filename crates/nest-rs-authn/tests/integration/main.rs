//! Integration tests for `nest-rs-authn`. Layout strictly mirrors `src/` (see CLAUDE.md).
//!
//! - This file is the only `tests/*.rs` binary; paths under `tests/` are modules.
//! - Shared fixtures live below at the suite root (`crate::…`), so every module
//!   in the tree mirrors a `src/` counterpart.
//! - Documented gaps: app e2e for live HTTP.

mod error;
mod jwt;
mod oauth;
mod passport;
mod password;
mod resource;

use nest_rs_mcp::{ServerHandler, mcp, rmcp, tool_handler, tool_router};
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

/// Read the `WWW-Authenticate` challenge off a response, failing with the
/// status when absent so a broken test says what actually came back.
pub fn challenge(resp: &poem::Response) -> String {
    resp.headers()
        .get(header::WWW_AUTHENTICATE)
        .unwrap_or_else(|| panic!("a {} must carry a challenge", resp.status()))
        .to_str()
        .expect("ascii")
        .to_owned()
}

/// A tool host with no tools: enough to mount `/mcp`, which is all a suite
/// asserting on the transport edge needs — the refusal it checks happens before
/// any tool would run.
///
/// rmcp's own host macros expand against the call site's scope; the `rmcp`
/// re-export imported above is what supplies that name without an `rmcp`
/// manifest entry.
#[mcp]
#[derive(Clone)]
pub struct EchoTool;

#[tool_router]
impl EchoTool {}

#[tool_handler]
impl ServerHandler for EchoTool {}
