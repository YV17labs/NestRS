//! MCP — `#[mcp]` mounts a Model Context Protocol server on the HTTP transport.
//!
//! MCP is **not a transport**, it is a graft on `HttpTransport` (the same
//! pattern as WS): `#[mcp]` on a struct emits an [`endpoint`] factory that
//! mounts under the HTTP server. Apps activate MCP by listing the
//! `#[mcp]`-decorated provider — there is no `<Transport>Module` activation
//! seam to import, and no `Transport` impl. [`McpModule`] exists only to
//! configure the streamable-HTTP server ([`McpConfig`]); it activates nothing.
//!
//! # Every capability, not just tools
//!
//! A host serves whatever it implements: `#[tool_router]`/`#[tool]` for tools,
//! `#[prompt_router]`/`#[prompt]` for prompts, and hand-written `ServerHandler`
//! methods for resources and templates, completion, logging levels,
//! subscriptions (SEP-2575), the `tasks/*` extension (SEP-2663), elicitation
//! and multi-round tool responses (SEP-2322), and custom methods. The protocol
//! types live under [`model`], the per-operation handles under [`service`], and
//! anything not re-exported here is reachable through [`rmcp`] itself.
//!
//! # Ambient request state reaches every operation
//!
//! rmcp dispatches each operation on its own spawned task, so a task-local
//! installed around the poem endpoint would not reach it. [`PropagatingHandler`]
//! closes that gap: the endpoint stashes the per-operation state in the request
//! extensions, rmcp forwards them as `http::request::Parts` into the operation's
//! `RequestContext`, and the handler re-installs everything *inside* the
//! dispatch. A handler method therefore gets the same transparency HTTP and
//! GraphQL have:
//!
//! * [`Scoped<T>::from_context`](Scoped) resolves an
//!   `#[injectable(scope = request)]` provider.
//! * The caller's ability is installed by the operation guard's
//!   [`around`](McpOperationGuard::around) — the same seam
//!   `GraphqlOperationGuard` uses, so "who installs the ability" has one answer
//!   on both transports.
//! * A `Repo`-backed body reads through the ambient executor, row-filtered by
//!   the caller's ability — provide `nest_rs_seaorm::mcp::McpDataContext`
//!   `as dyn McpToolContext` (what `AuthzMcpModule` does) to install it.
//!
//! This holds for **every** capability, not the tool call alone: the wrapper
//! delegates the whole `ServerHandler` surface, so a prompt fetch, a resource
//! read and a `tasks/get` are scoped and transacted exactly like `tools/call`.
//! The two documented exceptions — notifications and the long-lived
//! `subscriptions/listen` — are on [`McpToolContext::around`].
//!
//! The guard chain is enforced ahead of all that, in the order
//! [`resolve_operation_guard`] mounts: the app's registered
//! [`McpOperationGuard`] (`nest_rs_authz::mcp::McpAbilityBridge`, which answers
//! `401` without a valid token), else the global guard pool through
//! [`FallbackMcpGuard`], else **deny-all**. Without a registered
//! [`McpToolContext`] a `Repo`-backed body still fails **closed and loud**
//! (`Repo::conn` errors; `scope_for` denies every row), never a silent wrong
//! answer.
//!
#![warn(missing_docs)]

mod config;
mod context;
mod endpoint;
mod guard;
mod guards;
mod module;
mod propagate;
mod scope;

pub use config::McpConfig;
pub use context::{Captured, McpToolContext, OperationOutcome, OperationValue};
pub use endpoint::{McpMount, endpoint, resolve_operation_guard};
pub use guard::{BoxFuture, FallbackMcpGuard, McpOperationGuard};
pub use guards::AllowAllMcpGuard;
pub use module::{McpModule, McpSetup};
pub use propagate::PropagatingHandler;
/// Per-operation accessor for `#[injectable(scope = request)]` providers inside
/// an MCP tool method — the MCP mirror of `nest_rs_http::Scoped<T>`.
pub use scope::Scoped;

// --- The ergonomic surface: what a tool, prompt or resource host writes -----

pub use rmcp::{ErrorData as McpError, ServerHandler};
/// Host decorators. rmcp owns them; `nest-rs-mcp` re-exports them so a host
/// file imports one path. `#[tool_router]`/`#[prompt_router]` scan an inherent
/// `impl` for `#[tool]`/`#[prompt]` methods; `#[tool_handler]`/
/// `#[prompt_handler]` fill in the matching `ServerHandler` methods and stack on
/// one `impl ServerHandler` block when a host serves both.
pub use rmcp::{prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router};

pub use rmcp::handler::server::router::prompt::PromptRouter;
pub use rmcp::handler::server::router::tool::ToolRouter;
/// `Parameters<T>` deserializes a tool's typed input; `Json<T>` returns a typed
/// structured result (`structuredContent`, SEP-2106).
pub use rmcp::handler::server::wrapper::{Json, Parameters};

/// The two results almost every tool body names.
pub use rmcp::model::{CallToolResult, ContentBlock};

// --- The whole protocol, one module each ------------------------------------
//
// Re-exported wholesale rather than name by name. MCP grows a capability every
// few revisions — tasks (SEP-2663), subscriptions (SEP-2575), MRTR (SEP-2322),
// cache hints (SEP-2549) all landed in one rmcp major — and a hand-curated list
// of protocol types is a list that silently lags the protocol. Forwarding the
// modules means a capability rmcp gains is reachable through `nest_rs::mcp::`
// the day it ships, with no framework release in between.

/// Routers and argument wrappers behind the host decorators.
pub use rmcp::handler;
/// Every MCP protocol type: prompts, resources and templates, completion,
/// logging, elicitation, the `tasks/*` trio, subscription filters, discovery,
/// capabilities, content blocks, `_meta` and cache hints.
pub use rmcp::model;
/// Per-operation context and peer handles — `RequestContext`,
/// `NotificationContext`, `SubscriptionContext`, `Peer`, `RoleServer`. A
/// handler method takes one; a server-initiated call (sampling, elicitation,
/// progress) goes out through the peer.
pub use rmcp::service;
/// Transport plumbing, including the streamable-HTTP server this crate mounts.
pub use rmcp::transport;

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

/// The rmcp SDK itself, at the exact version the framework built against.
///
/// Two uses, both first-class:
///
/// * **Macro hygiene.** rmcp's `#[tool]` / `#[tool_router]` / `#[tool_handler]`
///   / `#[prompt]` family expands to bare `rmcp::` paths resolved against the
///   *call site's* scope. `use nest_rs::mcp::rmcp;` in a host file supplies that
///   name, so the host's manifest needs no `rmcp` entry and cannot drift from
///   the major the framework built against. It is one `use` beside the ones the
///   host already writes — deliberately explicit rather than emitted by
///   `#[mcp]`, because two hosts in one module would then collide on the name.
/// * **Reaching anything not re-exported above** — a newer rmcp item, or one
///   this crate has no opinion about.
pub use rmcp;

/// The wire-DTO shorthand — same decorator the HTTP layer uses, re-exported
/// here so a payload crossing this transport needs no `serde` of its own.
pub use nest_rs_core::input;

pub use nest_rs_mcp_macros::mcp;
