//! MCP — `#[mcp]` mounts a Model Context Protocol server on the HTTP transport,
//! and `#[tools]` declares the operations it serves.
//!
//! MCP is **not a transport**, it is a graft on `HttpTransport` (the same
//! pattern as WS): `#[mcp]` on a struct emits an [`endpoint`] factory that
//! mounts under the HTTP server. Apps activate MCP by listing the
//! `#[mcp]`-decorated provider — there is no `<Transport>Module` activation
//! seam to import, and no `Transport` impl. [`McpModule`] exists only to
//! configure the streamable-HTTP server ([`McpConfig`]) and name the app
//! ([`McpIdentity`]); it activates nothing.
//!
//! A host's `path` is the whole URL path: omit it for [`DEFAULT_PATH`]
//! (`/mcp`), or write one to serve a second endpoint. It is not a namespace the
//! host owns the way a `#[controller]`'s is — nothing nests under it, it names
//! the one endpoint the host joins, and peers writing the same path share it.
//!
//! # An operation declares the same layers a `#[query]` does
//!
//! `#[tools]` is a request-layer site, not just a router: each
//! `#[tool]` / `#[prompt]` takes `#[use_guards(...)]` / `#[force_guards(...)]`,
//! a **mandatory** access posture (`#[authorize(Action, Entity)]` or
//! `#[public]`), and a pipe on its arguments (`Parameters<Valid<T>>` /
//! `Parameters<Piped<P, T>>`). They expand in that order, so a caller the class
//! gate refuses never pays for validation.
//!
//! The host struct takes `#[use_guards(...)]` too, and the app-wide pool joins
//! them: all three scopes compose into one chain per site, deduplicated by
//! `TypeId` like every other layer family. The endpoint's
//! [`McpOperationGuard`] is not part of it and does not shorten it — that guard
//! is handed an HTTP request and runs `check_http`, while this chain is handed
//! the operation and runs `check_mcp`.
//!
//! Two things this transport does not inherit, and both follow from the
//! protocol rather than from a preference. Response masking has no selection set
//! to excuse a stripped **required** field, so it fails *closed*; declare
//! `unmasked` when a tool deliberately answers with a narrower projection. And
//! `bind = Service` has no MCP form: an operation takes one `Parameters<T>`
//! struct, not the named id argument the binding reads.
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

mod composite;
mod config;
mod context;
mod endpoint;
mod error;
mod guard;
mod guards;
mod host;
mod identity;
mod module;
mod operation;
mod propagate;
mod registry;
mod scope;

pub use composite::CompositeHandler;
pub use config::McpConfig;
pub use context::{Captured, McpToolContext, OperationOutcome, OperationValue};
pub use endpoint::{McpMount, endpoint, resolve_operation_guard};
pub use error::{Opaque, pipe_error, unresolvable_chain};
pub use guard::{BoxFuture, FallbackMcpGuard, McpOperationGuard};
pub use guards::AllowAllMcpGuard;
pub use host::McpHost;
pub use identity::{McpIdentity, ResolvedIdentity};
pub use module::{McpModule, McpOptions, McpSetup};
pub use operation::{McpOperationContext, McpOperationKind, current_container};
pub use propagate::PropagatingHandler;
pub use registry::{DEFAULT_PATH, McpHostMeta, endpoint_identity, hosts_on};
#[doc(hidden)]
pub use registry::{DefaultOperationLayers, DefaultToolRouter, register_host};
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
///   *call site's* scope. `#[tools]` carries that import for you, inside a
///   module named for the host — so two hosts in one file cannot
///   collide on the name, and the host's file writes no `use rmcp` at all. A
///   host that hand-writes `ServerHandler` is outside that expansion and imports
///   this itself, which is also what keeps its manifest free of an `rmcp` entry
///   that could drift from the major the framework built against.
/// * **Reaching anything not re-exported above** — a newer rmcp item, or one
///   this crate has no opinion about.
pub use rmcp;

/// The wire-DTO shorthand — same decorator the HTTP layer uses, re-exported
/// here so a payload crossing this transport needs no `serde` of its own.
pub use nest_rs_core::input;

/// The two pipe carriers an operation's arguments go through, re-exported so a
/// host file writing `Parameters<Valid<T>>` imports one path.
///
/// `Valid<T>` validates the deserialized arguments; `Piped<P, T>` runs any
/// [`Pipe`](nest_rs_pipes::Pipe) over them. `#[tools]` strips the carrier from
/// the wire signature — the tool's JSON Schema stays `T`'s — runs the pipe
/// before the body, and answers a rejection with `invalid_params`.
pub use nest_rs_pipes::{Piped, Valid};

pub use nest_rs_mcp_macros::{mcp, tools};
