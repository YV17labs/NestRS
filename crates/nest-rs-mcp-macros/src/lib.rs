//! `#[mcp]` / `#[tools]` decorators, re-exported by `nest-rs-mcp`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod mcp;
mod mcp_impl;

/// Behaves like `#[injectable]` for construction and emits a `Discoverable`
/// that attaches an `HttpEndpointMeta`. Its operations go under
/// [`macro@tools`] on the host's inherent impl; a host serving a hand-written
/// `ServerHandler` carries rmcp's own `#[tool_router]` / `#[tool_handler]`
/// impls instead. The factory runs per session, so per-session state stays
/// fresh.
///
/// Every argument is optional. `path` is the **whole URL path** — omit it to
/// serve `nest_rs_mcp::DEFAULT_PATH` (`/mcp`). Nothing nests under it: it names
/// the one endpoint the host joins, and peers that write the same one share it.
/// `name` / `title` override `McpOptions::server` per field for that endpoint;
/// two hosts on one path both declaring fails boot. `version` and
/// `instructions` are refused: they are declared once on `McpOptions::server`.
///
/// # Expands to
///
/// The struct unchanged, a `from_container` constructor, and an `impl
/// Discoverable` whose `register` hands the host to `nest_rs_mcp::__private::register_host`
/// — which resolves the path (the default when the host declared none), records
/// the contribution, and (for the first host on a path) attaches the exempt
/// `HttpEndpointMeta` that nests the rmcp endpoint behind the MCP operation
/// guard.
#[proc_macro_attribute]
pub fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(mcp::mcp(args, input).into()).into()
}

/// The operations half of an MCP host: its `#[tool]` and `#[prompt]` methods.
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
/// # Expands to
///
/// The authored impl re-emitted **untouched**, plus — inside a private child
/// module that carries rmcp's imports, so the host file needs none — a
/// delegating wrapper per operation carrying `#[tool(name = "…")]` (the
/// authored name stays the wire name), rmcp's routers with `pub(crate)`
/// visibility, and the `ServerHandler` impl with its `get_info`. Each wrapper
/// runs the guard chain, the posture's gate, the pipes, the authored method and
/// the response mask, in that order.
#[proc_macro_attribute]
pub fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(mcp::tools(args, input).into()).into()
}
