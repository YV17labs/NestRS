//! MCP decorator macro, re-exported by `nestrs-mcp`. The generated code uses
//! absolute paths (`::nestrs_mcp::*`, `::nestrs_http::*`, `::nestrs_core::*`,
//! `::poem::*`), so this crate does not depend on them — they resolve at the
//! call site. The implementation lives in `mcp`; this is the language-required
//! proc-macro entry.

use proc_macro::TokenStream;

mod mcp;

/// Mark a struct as an MCP server handler that mounts itself over HTTP.
///
/// Behaves like `#[injectable]` for construction (fields with `#[inject]`
/// resolved from the container, others default) and additionally emits an
/// `impl Discoverable` that attaches an `HttpEndpointMeta`. Listed in a
/// `#[module]`, the handler serves an MCP streamable-HTTP endpoint at `path`
/// with no `.mount()` call in `main.rs`.
///
/// The struct must carry the `rmcp` `#[tool_router]` / `#[tool_handler]`
/// impls — `nestrs_mcp::endpoint` requires `ServerHandler`. The factory runs
/// per session, so the handler is rebuilt from the container each time and
/// any per-session state stays fresh.
#[proc_macro_attribute]
pub fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    mcp::mcp(args, input)
}
