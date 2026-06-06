//! `#[mcp]` decorator, re-exported by `nestrs-mcp`. Emits absolute-path tokens.

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
#[proc_macro_attribute]
pub fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    mcp::mcp(args, input)
}
