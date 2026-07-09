//! `#[mcp]` decorator, re-exported by `nest-rs-mcp`. Emits absolute-path tokens.
use proc_macro::TokenStream;

mod mcp;

/// Mark a struct as an MCP server handler that self-mounts over HTTP.
///
/// Behaves like `#[injectable]` for construction and emits a `Discoverable`
/// that attaches an `HttpEndpointMeta` at `path`. The struct must carry the
/// `rmcp` `#[tool_router]` / `#[tool_handler]` impls. The factory runs per
/// session, so per-session state stays fresh.
///
/// ```ignore
/// #[mcp(path = "/mcp")]
/// struct MyHandler { #[inject] svc: Arc<MyService> }
/// ```
///
/// # Expands to
///
/// The struct unchanged, a `from_container` constructor, and an `impl
/// Discoverable` whose `register` attaches an exempt `HttpEndpointMeta` that
/// nests the rmcp endpoint (behind the MCP operation guard) at `path`.
///
/// ```ignore
/// struct MyHandler { /* … */ }
/// impl MyHandler { fn from_container(c) -> Self { /* … */ } }
/// impl ::nest_rs_core::Discoverable for MyHandler {
///     fn register(b) -> ContainerBuilder {
///         b.attach_meta::<MyHandler, ::nest_rs_http::HttpEndpointMeta>(
///             ::nest_rs_http::HttpEndpointMeta::new("/mcp", "mcp", |c, r| { /* nest guarded endpoint */ }).exempt(),
///         )
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    mcp::mcp(args, input)
}
