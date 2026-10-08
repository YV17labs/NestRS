//! Integration suite for `nest-rs-oauth-resource` — the RFC 9728 discovery
//! flow, booted through `TestApp`. Paths mirror `src/`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod controller;
mod interceptor;

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_mcp::{ServerHandler, mcp, rmcp, tool_handler, tool_router};
use poem::Request;
use poem::http::header;

/// Refuses every caller through `Denial::unauthorized`, the ordinary guard path.
#[injectable]
#[derive(Default)]
pub struct AlwaysUnauthorized;

impl Layer for AlwaysUnauthorized {}

#[async_trait]
impl Guard for AlwaysUnauthorized {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::unauthorized("missing bearer token"))
    }
}

impl HttpGuard for AlwaysUnauthorized {}

/// Read the `WWW-Authenticate` challenge off a response, panicking with its status when absent.
pub fn challenge(resp: &poem::Response) -> String {
    resp.headers()
        .get(header::WWW_AUTHENTICATE)
        .unwrap_or_else(|| panic!("a {} must carry a challenge", resp.status()))
        .to_str()
        .expect("ascii")
        .to_owned()
}

/// A tool host with no tools, enough to mount `/mcp`. rmcp's host macros expand
/// against the call site's scope, which the imported `rmcp` re-export supplies.
#[mcp]
#[derive(Clone)]
pub struct EchoTool;

#[tool_router(allow_empty)]
impl EchoTool {}

#[tool_handler]
impl ServerHandler for EchoTool {}
