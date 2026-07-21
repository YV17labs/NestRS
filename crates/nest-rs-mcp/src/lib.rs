//! MCP transport — `#[mcp]` mounts tools on the existing HTTP transport.
//!
//! Unlike HTTP / GraphQL / Queue / Schedule, this crate ships no `McpModule`
//! and no `Transport` impl. MCP is **not a transport**, it is a graft on
//! `HttpTransport` (the same pattern as WS): `#[mcp]` on a struct emits an
//! `endpoint()` factory that mounts under the HTTP server. Apps activate MCP
//! by listing the `#[mcp]`-decorated provider — no `<Transport>Module`
//! activation seam to import.
//!
//! # Ambient request state reaches a tool body
//!
//! rmcp dispatches each tool call on its own spawned task, so a task-local
//! installed around the poem endpoint would not reach it. [`PropagatingHandler`]
//! closes that gap: the endpoint stashes the per-operation state in the request
//! extensions, rmcp forwards them as `http::request::Parts` into the operation's
//! `RequestContext`, and the handler re-installs everything *inside* the
//! dispatch. A tool method therefore gets the same transparency HTTP and
//! GraphQL have:
//!
//! * [`Scoped<T>::from_context`](Scoped) resolves an
//!   `#[injectable(scope = request)]` provider.
//! * The caller's ability is installed by the operation guard's
//!   [`around`](McpOperationGuard::around) — the same seam
//!   `GraphqlOperationGuard` uses, so "who installs the ability" has one answer
//!   on both transports.
//! * A `Repo`-backed tool reads through the ambient executor, row-filtered by
//!   the caller's ability — provide `nest_rs_seaorm::mcp::McpDataContext`
//!   `as dyn McpToolContext` (what `AuthzMcpModule` does) to install it.
//!
//! The guard chain is enforced ahead of all that, in the order
//! [`resolve_operation_guard`] mounts: the app's registered
//! [`McpOperationGuard`] (`nest_rs_authz::mcp::McpAbilityBridge`, which answers
//! `401` without a valid token), else the global guard pool through
//! [`FallbackMcpGuard`], else **deny-all**. Without a registered
//! [`McpToolContext`] a `Repo`-backed tool still fails **closed and loud**
//! (`Repo::conn` errors; `scope_for` denies every row), never a silent wrong
//! answer.
//!
#![warn(missing_docs)]

mod context;
mod endpoint;
mod guard;
mod guards;
mod propagate;
mod scope;

pub use context::{Captured, McpToolContext, OperationOutcome};
pub use endpoint::{endpoint, endpoint_with_guard, resolve_operation_guard};
pub use guard::{BoxFuture, FallbackMcpGuard, McpOperationGuard};
pub use guards::AllowAllMcpGuard;
pub use propagate::PropagatingHandler;
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
