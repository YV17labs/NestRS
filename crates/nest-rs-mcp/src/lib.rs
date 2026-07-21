//! MCP transport — `#[mcp]` mounts tools on the existing HTTP transport.
//!
//! Unlike HTTP / GraphQL / Queue / Schedule, this crate ships no `McpModule`
//! and no `Transport` impl. MCP is **not a transport**, it is a graft on
//! `HttpTransport` (the same pattern as WS): `#[mcp]` on a struct emits an
//! `endpoint()` factory that mounts under the HTTP server. Apps activate MCP
//! by listing the `#[mcp]`-decorated provider — no `<Transport>Module`
//! activation seam to import.
//!
//! # 1.0 limitation — request-scoped state does not reach a tool body
//!
//! The guard chain is fully enforced: an MCP endpoint is **deny-all** without
//! an explicit [`McpOperationGuard`], and `McpAbilityBridge` returns `401`
//! without a valid token. What does **not** work in 1.0 is *transparent
//! per-operation ambient state inside a tool method*: rmcp dispatches each
//! tool call inside its own spawned `serve_directly` loop (both stateful and
//! stateless configs `tokio::spawn`), so the request-scope / ambient-executor /
//! ambient-ability task-locals installed around the poem endpoint never reach
//! the tool. In practice:
//!
//! * [`Scoped<T>::from_context`](Scoped) returns a clear `McpError` rather than
//!   resolving a `#[injectable(scope = request)]` provider.
//! * A `Repo`-backed tool (row-level filtering / masking) runs with **no
//!   ambient executor or ability** — it fails **closed and loud** (`Repo::conn`
//!   errors; `scope_for` denies every row with a `warn`), never a silent wrong
//!   answer.
//!
//! The transparent fix — a `PropagatingHandler` that re-installs scope +
//! executor + ability from the `http::request::Parts` rmcp injects into each
//! message's `RequestContext` — is tracked for a later release (ROADMAP,
//! *Transparent row-level filtering on MCP*). Until then, keep MCP tools to
//! non-`Repo`, non-request-scoped work, or resolve dependencies from the
//! singleton container.
#![warn(missing_docs)]

mod endpoint;
mod guard;
mod guards;
mod scope;

pub use endpoint::{endpoint, endpoint_with_guard};
pub use guard::{BoxFuture, McpOperationGuard};
pub use guards::AllowAllMcpGuard;
/// Per-operation accessor for `#[injectable(scope = request)]` providers inside
/// an MCP tool method — the MCP mirror of `nest_rs_http::Scoped<T>`.
pub use scope::Scoped;

pub use rmcp::handler::server::router::tool::ToolRouter;
pub use rmcp::handler::server::wrapper::Parameters;
pub use rmcp::model::{CallToolResult, ContentBlock};
pub use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

pub use nest_rs_mcp_macros::mcp;
