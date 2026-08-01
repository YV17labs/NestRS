//! `#[mcp]` — the tool host mounts on the HTTP transport, so its expansion
//! names the container, the endpoint meta and the operation-guard resolution.
//! None of those is a crate a tool host declares.

use nest_rs::mcp::{ServerHandler, mcp};

/// Minimal tool host: no tools, which is the smallest shape that still forces
/// the mount expansion. The rmcp `#[tool_router]` / `#[tool]` pair is not
/// exercised here — those are rmcp's own macros, and they expand against the
/// call site's prelude through the `nest_rs::mcp::rmcp` re-export.
#[mcp(path = "/hygiene-mcp")]
#[derive(Clone, Default)]
pub struct HygieneTool;

impl ServerHandler for HygieneTool {}
