//! `#[mcp]` / `#[tools]` decorators, re-exported by `nest-rs-mcp`. Emits
//! absolute-path tokens.
//!
//! The same struct/impl split every other edge has — `#[controller]`/`#[routes]`,
//! `#[gateway]`/`#[messages]`, `#[resolver]`/`#[operations]` — with one decorator
//! per item shape.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod mcp;
mod mcp_impl;

/// Mark a **struct** as an MCP host that self-mounts over HTTP.
///
/// Behaves like `#[injectable]` for construction and emits a `Discoverable`
/// that attaches an `HttpEndpointMeta`. Its operations go under
/// [`macro@tools`] on the host's inherent impl; a host serving a hand-written
/// `ServerHandler` carries rmcp's own `#[tool_router]` / `#[tool_handler]`
/// impls instead. The factory runs per session, so per-session state stays
/// fresh.
///
/// Every argument is optional. `path` is the **whole URL path** — omit it to
/// serve `nest_rs_mcp::DEFAULT_PATH` (`/mcp`), which is what a feature
/// contributing tools to this app's server wants. Unlike a controller's, the
/// path is not a namespace the host owns: nothing nests under it, it names the
/// one endpoint the host joins, and peers that write the same one share it.
/// `name` / `title` declare which endpoint stands apart from the app's default,
/// overriding `McpOptions::server` per field; two hosts on one path both
/// declaring fails boot. Neither `version` nor `instructions` is an argument:
/// both describe the *server* — a feature library knows neither the binary's
/// version nor, on a shared endpoint, the whole surface — so they are declared
/// once on `McpOptions::server`, and what each tool does belongs to its own
/// `#[tool(description = "…")]`.
///
/// ```ignore
/// #[mcp]
/// struct MyHandler { #[inject] svc: Arc<MyService> }
///
/// #[mcp(path = "/mcp/posts", name = "assistant-posts")]
/// struct PostsHandler { #[inject] svc: Arc<PostsService> }
/// ```
///
/// # Why the host decorator is not named for its role
///
/// Every other host decorator is — `#[controller]`, `#[resolver]`,
/// `#[gateway]`, `#[processor]`. Here the role word is spoken by the *impl*
/// half, [`macro@tools`], because that is what a host's methods are; the struct
/// keeps the protocol's name. `#[mcp]` also cannot be misread as rmcp's
/// `#[tool]`, which this crate re-exports and which the same file carries. The
/// role word is elsewhere too — the file (`tool.rs`) and the module
/// (`<Feature>McpModule`). Accepted asymmetry, not an oversight.
///
/// # Expands to
///
/// The struct unchanged, a `from_container` constructor, and an `impl
/// Discoverable` whose `register` hands the host to `nest_rs_mcp::register_host`
/// — which resolves the path (the default when the host declared none), records
/// the contribution, and (for the first host on a path) attaches the exempt
/// `HttpEndpointMeta` that nests the rmcp endpoint behind the MCP operation
/// guard.
///
/// ```ignore
/// struct MyHandler { /* … */ }
/// impl MyHandler { fn from_container(c) -> Self { /* … */ } }
/// impl ::nest_rs_core::Discoverable for MyHandler {
///     fn register(b) -> ContainerBuilder {
///         ::nest_rs_mcp::register_host::<Self>(
///             b, "", "MyHandler",
///             ::nest_rs_mcp::McpIdentity::declared(None, None),
///             |c| Arc::new(Self::from_container(c)),
///             || Self::tool_router().list_all(),
///         )
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(mcp::mcp(args, input).into()).into()
}

/// Declare an `#[mcp]` host's operations on its **inherent impl block** — the
/// MCP counterpart of `#[routes]`, `#[messages]` and `#[operations]`, named for
/// what it collects. It carries `#[prompt]` methods as well as `#[tool]` ones:
/// rmcp routes both through the one `ServerHandler` this expansion writes, so
/// they are one host's operations, not two blocks.
///
/// Takes no arguments — path and identity are `#[mcp]`'s, on the struct.
///
/// It absorbs rmcp's three-block shape (`#[tool_router]`, `#[prompt_router]`,
/// `#[tool_handler]`/`#[prompt_handler]` + `get_info`) into generated code, and
/// gives each operation the request layers every other edge has:
/// `#[use_guards]` / `#[force_guards]`, a **mandatory** posture
/// (`#[authorize(Action, Entity)]` or `#[public]`), per-argument pipes inside
/// `Parameters<…>`, and response masking. The advertised capabilities are
/// **derived** from the roles present, so a host cannot route what it forgot to
/// declare.
///
/// A host that hand-writes `ServerHandler` (resources, completion) writes rmcp
/// directly and has no `#[tools]` block at all — `#[tools]` on a trait impl is
/// a compile error saying so.
///
/// ```ignore
/// #[mcp]
/// struct MyHandler { #[inject] svc: Arc<MyService> }
///
/// #[tools]
/// impl MyHandler {
///     #[tool(description = "…")]
///     #[authorize(Read, users::Entity)]
///     async fn find(&self, Parameters(p): Parameters<Valid<FindDto>>) -> Result<String, McpError> { /* … */ }
/// }
/// ```
///
/// # Expands to
///
/// The authored impl re-emitted **untouched**, plus — inside a private child
/// module that carries rmcp's imports, so the host file needs none — a
/// delegating wrapper per operation carrying `#[tool(name = "…")]` (the
/// authored name stays the wire name), rmcp's routers with `pub(crate)`
/// visibility, and the `ServerHandler` impl with its `get_info`.
///
/// ```ignore
/// impl MyHandler { /* the authored methods, unchanged */ }
///
/// mod __nestrs_my_handler_mcp {
///     use ::nest_rs_mcp::rmcp;
///     #[rmcp::tool_router(vis = "pub(crate)")]
///     impl super::MyHandler {
///         #[tool(name = "find")]
///         async fn __nestrs_find(&self, /* wire signature */) -> Result<CallToolResult, McpError> {
///             /* guard chain → posture gate → pipes → self.find(..) → response mask */
///         }
///     }
///     #[rmcp::tool_handler]
///     impl rmcp::ServerHandler for super::MyHandler {
///         fn get_info(&self) -> ServerInfo { /* capabilities derived from the roles present */ }
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(mcp::tools(args, input).into()).into()
}
