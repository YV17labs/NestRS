//! MCP — `#[mcp]` mounts a Model Context Protocol server on the HTTP transport,
//! and `#[tools]` declares the operations it serves.
//!
//! An app activates MCP by importing an adapter that lists an `#[mcp]` provider,
//! which brings [`McpModule`] bare, reading the streamable-HTTP server's
//! [`McpConfig`] and bringing the HTTP transport; the root's `for_root` names the
//! app ([`McpIdentity`]). A host's `path` is the whole URL path,
//! [`DEFAULT_PATH`] (`/mcp`) when omitted; nothing nests under it, and hosts
//! writing the same path share one endpoint.
//!
//! # An operation runs the layers every other edge runs
//!
//! Each request is one `mcp.operation` unit on core's pipeline, and a
//! `#[tool]`, `#[prompt]`, `#[resource]` or `#[resources]` is a site of its
//! own inside it, taking
//! `#[use_guards(...)]` / `#[force_guards(...)]`, an access posture
//! (`#[authorize(Action, Entity)]` or `#[public]`, optional where the endpoint's
//! guards cover the operation and mandatory on an endpoint the app opened),
//! `#[use_exception_filters(...)]` and `#[use_interceptors(...)]`. Its tail then
//! holds each argument it decoded to the rules its type declares, runs a pipe
//! on its arguments (`Parameters<Piped<P>>`), and reads its `Scoped<T>` and
//! `Ambient<T>` parameters, in that order. The host's layers and the
//! app-wide ones join each family once per site, deduplicated by `TypeId`; the
//! guard stage runs each guard's `check`, the guards at the endpoint's request
//! running `check_http` apart from it.
//!
//! A tool's answer is written inside its site, so each wire row it carries is
//! written for the caller that site answers: a row that caller may not read
//! refuses the operation `FORBIDDEN`, and a column its grant withholds reads
//! `null` in the structured content, as the output schema says it may.
//! `bind = Service` loads the row a `#[resource]`'s one template variable names;
//! a tool, a prompt and a listing name no id and refuse it.
//!
//! # Every capability, not just tools
//!
//! Under `#[tools]` a host serves tools, prompts and resources — a
//! `#[resource(template = "post://{id}")]` read takes the RFC 6570 template's
//! variables as typed arguments, a `#[resources]` method lists what can be
//! read — and the capabilities it advertises are derived from them. What no
//! decorator declares — completion, logging levels, subscriptions — a host
//! writes by hand on rmcp's `ServerHandler`, with no `#[tools]` block. The
//! protocol types live under [`model`], the per-operation handles under
//! [`service`], and anything not re-exported here under [`rmcp`].
//!
//! # Ambient request state reaches every operation
//!
//! rmcp dispatches each operation on its own task; [`PropagatingHandler`]
//! re-installs the `/mcp` request's context inside that dispatch, for every
//! capability, so a handler method gets:
//!
//! * [`Scoped<T>`](nest_rs_core::Scoped), a parameter, for an
//!   `#[injectable(scope = request)]` provider;
//! * what the request's guards and layers made ambient — the caller's
//!   ability — and its trace;
//! * a `Repo`-backed body reading through the ambient executor of the
//!   operation's own unit of work, opened through the bound
//!   `nest_rs_database::UnitOfWork`, row-filtered by the caller's ability where
//!   its posture names what it reads, and reading nothing a grant guards where
//!   it names nothing; a hand-written `ServerHandler`, which writes no posture,
//!   reads under a principal's ability and masks its answer itself.
//!
//! Notifications and `subscriptions/listen` run no unit of work, documented
//! on [`PropagatingHandler`].
//!
//! The HTTP transport runs the endpoint's request guards in front of it: the
//! app's global guard pool and the `EndpointGuards<McpView, _>` its modules
//! declare, each once — and with neither, **deny-all**, unless the app opens
//! the endpoint in code ([`McpAccess::Public`]). Without a bound unit of work
//! a `Repo`-backed body fails closed (every `Repo` call errors), and without an
//! ability `scope_for` denies every row.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — MCP operations, hosts and the merged endpoint.
pub const TARGET: &str = "nest_rs::mcp";

mod access;
mod ambient;
mod composite;
mod config;
mod endpoint;
mod error;
mod guard;
mod host;
mod identity;
mod module;
mod operation;
mod parameters;
mod propagate;
mod registry;
mod resource;
mod scope;
mod sessions;
#[cfg(test)]
mod testing;
pub mod unit;
mod uri_template;
mod view;

pub use access::McpAccess;
pub use composite::CompositeHandler;
pub use config::McpConfig;
pub use error::{Opaque, pipe_error, problem_error};
pub use guard::{BoxFuture, denial_to_mcp_error};
pub use host::McpHost;
pub use identity::{McpIdentity, ResolvedIdentity};
pub use module::{McpModule, McpSetup};
pub use operation::McpOperationKind;
pub use propagate::PropagatingHandler;
pub use registry::{DEFAULT_PATH, McpHostMeta, endpoint_identity};
pub use view::McpView;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::ambient::ambient_parameter;
    pub use crate::endpoint::{McpMount, endpoint};
    pub use crate::error::answered_tool;
    pub use crate::error::{
        ErrorReport, ErrorReportChain, OperationAnswer, OperationFailure, answer_of,
        resource_not_found,
    };
    pub use crate::identity::declared_identity;
    pub use crate::module::RUNTIME;
    pub use crate::operation::{
        OPERATION_LAYERS, OperationGuards, description_is_blank, operation_guard_stage,
        operation_view, run_operation, served_path,
    };
    pub use crate::registry::{
        DeclaredOperation, DeclaredRoutes, DefaultDeclaredRoutes, DefaultOperationLayers,
        DefaultPromptRouter, DefaultToolRouter, ENDPOINT, ToolsHost, declare_endpoint, hosts_on,
        register_host,
    };
    pub use crate::resource::{
        MimeProbe, MimeProbeFallback, MimeProbeReads, ResourceBody, ResourceListing, bound_subject,
    };
    pub use crate::scope::scoped_parameter;
    pub use crate::uri_template::{TemplateMatch, TemplatePart, UriTemplate};
    pub use crate::view::served_operation;
    pub use nest_rs_pipes::__private::{DeclaredRules, NoRules, OptionalRules, ValidateProbe};
    pub use rmcp::handler::server::prompt::IntoGetPromptResult;
    pub use rmcp::handler::server::tool::IntoCallToolResult;
    pub use rmcp::model::CallToolResponse;
}

/// What a host's `get_info` reports about itself and advertises.
pub use rmcp::model::ServerConfig;
pub use rmcp::{ErrorData as McpError, ServerHandler};

/// `Parameters<T>` deserializes an operation's typed input — the framework's
/// own, refusing without the value; `Json<T>` returns a typed structured result
/// (`structuredContent`, SEP-2106).
pub use parameters::Parameters;
pub use rmcp::handler::server::wrapper::Json;

/// Every MCP protocol type: prompts, resources and templates, completion,
/// logging, elicitation, the `tasks/*` trio, subscription filters, discovery,
/// capabilities, content blocks, `_meta` and cache hints.
pub use rmcp::model;
/// Per-operation context and peer handles — `RequestContext`,
/// `NotificationContext`, `SubscriptionContext`, `Peer`, `RoleServer`. A
/// handler method takes one; a notification (progress) goes out through the
/// peer, and so does a request (sampling, elicitation) below protocol
/// `2026-07-28`, which refuses it: there an `InputRequiredResult` asks.
pub use rmcp::service;

/// The rmcp SDK itself, at the exact version the framework built against, for
/// a host writing `ServerHandler` by hand: its host macros, routers and
/// transport plumbing are reached here.
///
/// rmcp's macros expand to bare `rmcp::` paths: `#[tools]` imports this for them,
/// and a host hand-writing `ServerHandler` imports it itself rather than adding
/// an `rmcp` manifest entry that could drift from this major.
pub use rmcp;

/// Mark a struct as an MCP host that self-mounts over HTTP.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::{Discoverable, Discovery};
/// use nest_rs_mcp::{DEFAULT_PATH, McpHostMeta, mcp};
/// # use nest_rs_core::{injectable, module};
/// # use nest_rs_mcp::{McpError, McpIdentity, McpAccess, McpModule, tools};
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
/// #     McpModule::for_root((
/// #         McpAccess::Public,
/// #         McpIdentity {
/// #             name: Some("assistant".into()),
/// #             version: Some("1.0.0".into()),
/// #             ..Default::default()
/// #         },
/// #     )),
/// # ], providers = [
/// #     MyService,
/// #     PostsService,
/// #     MyHandler,
/// #     PostsHandler,
/// # ])]
/// # struct AppModule;
///
/// fn implements<T: Discoverable>() {}
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// implements::<MyHandler>();
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// let hosts = Discovery::new(app.container()).meta::<McpHostMeta>();
/// let on = |path: &str| hosts.iter().filter(|host| host.meta.path() == path).count();
/// assert_eq!((on(DEFAULT_PATH), on("/mcp/posts")), (1, 1));
///
/// let handshake = result(&initialize(app.http(), "/mcp/posts", None).await);
/// assert_eq!(handshake["result"]["serverInfo"]["name"], "assistant-posts");
/// # Ok(())
/// # }
/// ```
pub use nest_rs_mcp_macros::mcp;

/// Declare an `#[mcp]` host's operations on its inherent impl block. A tool a guard
/// identifying its caller covers at the endpoint needs no posture;
/// `#[authorize(Action, Subject)]` adds a gate and a reply mask, which are
/// `nest-rs-authz`'s, and `#[public]` admits a caller with no credential, the one an
/// endpoint the app opened takes when it declares no gate: bare it reads nothing a
/// grant guards, and `#[public(Action, Subject)]` names what that caller reads. Its
/// arguments are held to the rules their type declares, a refusal answered
/// `invalid_params` before the body runs.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_mcp::{McpError, Parameters, ServerHandler, mcp, tools};
/// use nest_rs_core::input;
/// # use nest_rs_core::{injectable, module};
/// # use nest_rs_core::Discovery;
/// # use nest_rs_mcp::{DEFAULT_PATH, McpAccess, McpHostMeta, McpModule};
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
///     async fn find(&self, Parameters(p): Parameters<FindDto>) -> Result<String, McpError> {
///         Ok(p.name)
///     }
/// }
///
/// fn implements<T: ServerHandler>() {}
/// # #[module(imports = [McpModule::for_root(McpAccess::Public)], providers = [MyService, MyHandler])]
/// # struct AppModule;
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// implements::<MyHandler>();
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// let hosts = Discovery::new(app.container()).meta::<McpHostMeta>();
/// assert_eq!(hosts[0].meta.declared_tools()[0].name, "find");
///
/// let arguments = serde_json::json!({ "name": "ada" });
/// let body = call_tool_with(app.http(), DEFAULT_PATH, "find", None, arguments).await;
/// assert_eq!(result(&body)["result"]["content"][0]["text"], "ada");
///
/// let blank = serde_json::json!({ "name": "" });
/// let body = call_tool_with(app.http(), DEFAULT_PATH, "find", None, blank).await;
/// assert_eq!(result(&body)["error"]["code"], -32602);
/// # Ok(())
/// # }
/// ```
///
/// A resource is read at a URI its template writes, each `{variable}` an
/// argument parsed with `FromStr`; `None` is a resource the caller may not
/// read, answered as one no template reads:
///
/// ```
/// use nest_rs_mcp::{McpError, mcp, tools};
/// # use nest_rs_core::module;
/// # use nest_rs_mcp::{DEFAULT_PATH, McpAccess, McpModule};
/// # use nest_rs_testing::mcp::{call_method, open_session, result};
///
/// #[mcp]
/// #[derive(Default)]
/// struct NotesHost;
///
/// #[tools]
/// impl NotesHost {
///     #[resource(template = "note://{id}", description = "One note's text.")]
///     #[public]
///     async fn note(&self, id: u32) -> Result<Option<String>, McpError> {
///         Ok((id == 1).then(|| "the first note".to_owned()))
///     }
/// }
/// # #[module(imports = [McpModule::for_root(McpAccess::Public)], providers = [NotesHost])]
/// # struct AppModule;
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
/// # let session = open_session(app.http(), DEFAULT_PATH, None).await;
/// # let read = |uri: &'static str| {
/// #     let (http, session) = (app.http(), session.clone());
/// #     async move {
/// #         let params = serde_json::json!({ "uri": uri });
/// #         result(&call_method(http, DEFAULT_PATH, &session, None, "resources/read", params).await)
/// #     }
/// # };
///
/// let found = read("note://1").await;
/// assert_eq!(found["result"]["contents"][0]["text"], "the first note");
///
/// let missing = read("note://2").await;
/// assert_eq!(missing["error"]["message"], "Resource not found");
/// # Ok(())
/// # }
/// ```
pub use nest_rs_mcp_macros::tools;
