//! Integration suite for `nest-rs-oauth-resource` — the RFC 9728 discovery
//! flow, booted through `TestApp`. Paths mirror `src/`.
//!
//! Shared fixtures live here at the suite root (`crate::…`); every module below
//! mirrors a `src/` counterpart.

mod controller;
mod interceptor;

use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_mcp::{ServerHandler, mcp, rmcp, tool_handler, tool_router};
use poem::Request;
use poem::http::header;

/// Refuses every caller, which is what an unauthenticated client meets on any
/// edge. It denies through `Denial::unauthorized` — the framework's ordinary
/// guard path, whose `401` carries `problem+json` and, before this capability,
/// no challenge whatsoever.
///
/// Shared: the HTTP walk and the WS upgrade need the same refusal, and two
/// copies of it drift into two different denial messages for one condition.
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
