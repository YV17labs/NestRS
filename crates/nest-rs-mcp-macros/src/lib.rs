//! `#[mcp]` decorator, re-exported by `nest-rs-mcp`. Emits absolute-path tokens.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod mcp;
mod mcp_impl;

/// Mark a struct as an MCP server handler that self-mounts over HTTP.
///
/// Behaves like `#[injectable]` for construction and emits a `Discoverable`
/// that attaches an `HttpEndpointMeta`. The struct must carry the `rmcp`
/// `#[tool_router]` / `#[tool_handler]` impls. The factory runs per session, so
/// per-session state stays fresh.
///
/// Every argument is optional. `path` is the **whole URL path** — omit it to
/// serve `nest_rs_mcp::DEFAULT_PATH` (`/mcp`), which is what a feature
/// contributing tools to this app's server wants. Unlike a controller's, the
/// path is not a namespace the host owns: nothing nests under it, it names the
/// one endpoint the host joins, and peers that write the same one share it.
/// `name` / `version` / `title` declare which endpoint stands apart from the
/// app's default, overriding `McpOptions::server` per field; `version` needs a
/// `name` beside it, and two hosts on one path both declaring fails boot.
/// `instructions` is **not** an argument: it describes the *server*, so it is
/// declared once on `McpOptions::server`, and what each tool does belongs to
/// its own `#[tool(description = "…")]`.
///
/// ```ignore
/// #[mcp]
/// struct MyHandler { #[inject] svc: Arc<MyService> }
///
/// #[mcp(path = "/mcp/posts", name = "assistant-posts")]
/// struct PostsHandler { #[inject] svc: Arc<PostsService> }
/// ```
///
/// # Why not `#[tools]`?
///
/// Every other host decorator is named for its role — `#[controller]`,
/// `#[resolver]`, `#[gateway]`, `#[processor]` — which would argue for
/// `#[tools]` on a `tool.rs`. It is **deliberately** `#[mcp]`: this crate
/// re-exports rmcp's own `#[tool]`, and the tool host file carries both. A
/// `#[tools]` sitting one letter from the `#[tool]` beneath it would read as a
/// typo at every glance, while `#[mcp]` cannot be confused with anything. The
/// role word stays where it is unambiguous — the file name (`tool.rs`) and the
/// module (`<Feature>McpModule`). Accepted asymmetry, not an oversight.
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
///             ::nest_rs_mcp::McpIdentity::declared(None, None, None),
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
