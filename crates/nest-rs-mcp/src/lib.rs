//! MCP — `#[mcp]` mounts a Model Context Protocol server on the HTTP transport,
//! and `#[tools]` declares the operations it serves.
//!
//! An app activates MCP by listing an `#[mcp]` provider; [`McpModule`] only
//! configures the streamable-HTTP server ([`McpConfig`]) and names the app
//! ([`McpIdentity`]). A host's `path` is the whole URL path, [`DEFAULT_PATH`]
//! (`/mcp`) when omitted; nothing nests under it, and hosts writing the same
//! path share one endpoint.
//!
//! # An operation declares the same layers a `#[query]` does
//!
//! Each `#[tool]` / `#[prompt]` takes `#[use_guards(...)]` / `#[force_guards(...)]`,
//! a **mandatory** access posture (`#[authorize(Action, Entity)]` or
//! `#[public]`), and a pipe on its arguments (`Parameters<Valid<T>>` /
//! `Parameters<Piped<P, T>>`), run in that order. The host's `#[use_guards]` and
//! the app-wide pool join one chain per site, deduplicated by `TypeId`, which
//! runs `check_mcp`; the endpoint's [`McpOperationGuard`] runs `check_http`
//! apart from it.
//!
//! Response masking fails *closed* on a stripped **required** field — declare
//! `unmasked` when a tool deliberately answers with a narrower projection — and
//! `bind = Service` has no MCP form: an operation takes one `Parameters<T>`.
//!
//! # Every capability, not just tools
//!
//! A host serves `#[tool_router]`/`#[tool]` tools, `#[prompt_router]`/`#[prompt]`
//! prompts, and whatever else it implements by hand on `ServerHandler`. The
//! protocol types live under [`model`], the per-operation handles under
//! [`service`], and anything not re-exported here under [`rmcp`].
//!
//! # Ambient request state reaches every operation
//!
//! rmcp dispatches each operation on its own task; [`PropagatingHandler`]
//! re-installs the per-operation state inside that dispatch, for every
//! capability, so a handler method gets:
//!
//! * [`Scoped<T>::from_context`](Scoped) for an `#[injectable(scope = request)]`
//!   provider;
//! * the caller's ability, installed by the operation guard's
//!   [`around`](McpOperationGuard::around);
//! * a `Repo`-backed body reading through the ambient executor, row-filtered by
//!   the caller's ability, once `nest_rs_seaorm::mcp::McpDataContext` is provided
//!   `as dyn McpToolContext` (what `AuthzMcpModule` does).
//!
//! Notifications and `subscriptions/listen` are the exceptions, documented on
//! [`McpToolContext::around`].
//!
//! The endpoint guard is, in the order [`resolve_operation_guard`] mounts: the
//! app's registered [`McpOperationGuard`], else the global guard pool through
//! its fallback slot, else **deny-all**. Without a registered
//! [`McpToolContext`] a `Repo`-backed body fails closed (`Repo::conn` errors;
//! `scope_for` denies every row).
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — MCP operations, hosts and the merged endpoint.
pub const TARGET: &str = "nest_rs::mcp";

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
mod parameters;
mod propagate;
mod registry;
mod scope;
pub mod unit;

pub use composite::CompositeHandler;
pub use config::McpConfig;
pub use context::{Captured, McpToolContext, OperationOutcome, OperationValue};
pub use endpoint::{McpMount, endpoint, resolve_operation_guard};
pub use error::{Opaque, pipe_error, problem_error, unresolvable_chain};
pub use guard::{BoxFuture, McpOperationGuard};
pub use guards::AllowAllMcpGuard;
pub use host::McpHost;
pub use identity::{McpIdentity, ResolvedIdentity};
pub use module::{McpModule, McpOptions, McpSetup};
pub use operation::{McpOperationContext, McpOperationKind, current_container};
pub use propagate::PropagatingHandler;
pub use registry::{DEFAULT_PATH, McpHostMeta, endpoint_identity, hosts_on};
/// Per-operation accessor for `#[injectable(scope = request)]` providers inside
/// an MCP tool method — the MCP mirror of `nest_rs_http::Scoped<T>`.
pub use scope::Scoped;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::error::{OperationAnswer, refused};
    pub use crate::guard::FallbackMcpGuard;
    pub use crate::identity::declared_identity;
    pub use crate::operation::description_is_blank;
    pub use crate::registry::{DefaultOperationLayers, DefaultToolRouter, register_host};
}

pub use rmcp::model::ProtocolVersion;
/// What a host reports about itself and advertises.
pub use rmcp::model::{ServerCapabilities, ServerConfig};
pub use rmcp::{ErrorData as McpError, ServerHandler};
/// Host decorators, rmcp's own. `#[tool_router]`/`#[prompt_router]` scan an inherent
/// `impl` for `#[tool]`/`#[prompt]` methods; `#[tool_handler]`/
/// `#[prompt_handler]` fill in the matching `ServerHandler` methods and stack on
/// one `impl ServerHandler` block when a host serves both.
pub use rmcp::{prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router};

/// `Parameters<T>` deserializes an operation's typed input — the framework's
/// own, refusing without the value; `Json<T>` returns a typed structured result
/// (`structuredContent`, SEP-2106).
pub use parameters::Parameters;
pub use rmcp::handler::server::router::prompt::PromptRouter;
pub use rmcp::handler::server::router::tool::ToolRouter;
pub use rmcp::handler::server::wrapper::Json;

/// The two results almost every tool body names.
pub use rmcp::model::{CallToolResult, ContentBlock};

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
/// rmcp's macros expand to bare `rmcp::` paths: `#[tools]` imports this for them,
/// and a host hand-writing `ServerHandler` imports it itself rather than adding
/// an `rmcp` manifest entry that could drift from this major.
pub use rmcp;

/// The wire-DTO shorthand — same decorator the HTTP layer uses, re-exported
/// here so a payload crossing this transport needs no `serde` of its own.
pub use nest_rs_core::input;

/// The two pipe carriers an operation's arguments go through.
///
/// `Valid<T>` validates the deserialized arguments; `Piped<P, T>` runs any
/// [`Pipe`](nest_rs_pipes::Pipe) over them. `#[tools]` strips the carrier from
/// the wire signature — the tool's JSON Schema stays `T`'s — and answers a
/// rejection with `invalid_params`.
pub use nest_rs_pipes::{Piped, Valid};

/// Mark a struct as an MCP host that self-mounts over HTTP.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::Discoverable;
/// use nest_rs_mcp::{DEFAULT_PATH, hosts_on, mcp};
/// # use nest_rs_core::{injectable, module};
/// # use nest_rs_mcp::{
/// #     AllowAllMcpGuard, McpError, McpIdentity, McpModule, McpOperationGuard, McpOptions, tools,
/// # };
/// # use nest_rs_testing::mcp::{initialize, result};
/// #
/// # #[injectable]
/// # #[derive(Default)]
/// # struct MyService;
/// #
/// # #[injectable]
/// # #[derive(Default)]
/// # struct PostsService;
///
/// #[mcp]
/// struct MyHandler {
///     #[inject]
///     svc: Arc<MyService>,
/// }
///
/// #[mcp(path = "/mcp/posts", name = "assistant-posts")]
/// struct PostsHandler {
///     #[inject]
///     svc: Arc<PostsService>,
/// }
/// #
/// # #[tools]
/// # impl MyHandler {
/// #     #[tool(description = "Answer a ping.")]
/// #     #[public]
/// #     async fn ping(&self) -> Result<String, McpError> {
/// #         Ok("pong".into())
/// #     }
/// # }
/// #
/// # #[tools]
/// # impl PostsHandler {
/// #     #[tool(description = "Count the posts.")]
/// #     #[public]
/// #     async fn count_posts(&self) -> Result<String, McpError> {
/// #         Ok("0".into())
/// #     }
/// # }
/// #
/// # #[module(imports = [
/// #     McpModule::for_root(McpOptions {
/// #         server: Some(McpIdentity::new("assistant", "1.0.0")),
/// #         ..Default::default()
/// #     }),
/// # ], providers = [
/// #     MyService,
/// #     PostsService,
/// #     MyHandler,
/// #     PostsHandler,
/// #     AllowAllMcpGuard as dyn McpOperationGuard,
/// # ])]
/// # struct AppModule;
///
/// fn implements<T: Discoverable>() {}
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// implements::<MyHandler>();
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// assert_eq!(hosts_on(app.container(), DEFAULT_PATH).len(), 1);
/// assert_eq!(hosts_on(app.container(), "/mcp/posts").len(), 1);
///
/// let handshake = result(&initialize(app.http(), "/mcp/posts", None).await);
/// assert_eq!(handshake["result"]["serverInfo"]["name"], "assistant-posts");
/// # Ok(())
/// # }
/// ```
pub use nest_rs_mcp_macros::mcp;

/// Declare an `#[mcp]` host's operations on its inherent impl block. Each tool
/// declares its posture, `#[public]` or `#[authorize(Action, Entity)]`; the gate
/// and the reply mask `#[authorize]` emits run in `nest_rs_authz::mcp`.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_mcp::{McpError, Parameters, ServerHandler, Valid, input, mcp, tools};
/// # use nest_rs_core::{injectable, module};
/// # use nest_rs_mcp::{AllowAllMcpGuard, DEFAULT_PATH, McpOperationGuard, hosts_on};
/// # use nest_rs_testing::mcp::{call_tool_with, result};
/// #
/// # #[injectable]
/// # #[derive(Default)]
/// # struct MyService;
///
/// #[input]
/// struct FindDto {
///     #[validate(length(min = 1))]
///     name: String,
/// }
///
/// #[mcp]
/// struct MyHandler {
///     #[inject]
///     svc: Arc<MyService>,
/// }
///
/// #[tools]
/// impl MyHandler {
///     #[tool(description = "Find a user by name.")]
///     #[public]
///     async fn find(&self, Parameters(p): Parameters<Valid<FindDto>>) -> Result<String, McpError> {
///         Ok(p.into_inner().name)
///     }
/// }
///
/// fn implements<T: ServerHandler>() {}
/// # #[module(providers = [MyService, MyHandler, AllowAllMcpGuard as dyn McpOperationGuard])]
/// # struct AppModule;
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// implements::<MyHandler>();
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// let declared = hosts_on(app.container(), DEFAULT_PATH)[0].declared_tools();
/// assert_eq!(declared[0].name, "find");
///
/// let arguments = serde_json::json!({ "name": "ada" });
/// let body = call_tool_with(app.http(), DEFAULT_PATH, "find", None, arguments).await;
/// assert_eq!(result(&body)["result"]["content"][0]["text"], "ada");
/// # Ok(())
/// # }
/// ```
pub use nest_rs_mcp_macros::tools;
