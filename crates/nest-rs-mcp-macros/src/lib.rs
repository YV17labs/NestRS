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
/// A `#[meta(EXPR)]` below it is declared for every operation of the host. The
/// struct is concrete: each operation's site is a `static`, which no parameter
/// of a generic struct reaches.
///
/// # Expands to
///
/// The struct unchanged, a hidden constructor, the host's layer lists the impl
/// half's sites read, and an `impl Discoverable` whose `register` hands the
/// host to the MCP host registry — which resolves the path (the default when
/// the host declared none), records the contribution with its operations'
/// sites, and (for the first host on a path) attaches the `HttpEndpointMeta`
/// that mounts the rmcp endpoint behind the request's guards.
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
/// (`#[authorize(Action, Entity)]` or `#[public]`), `#[use_exception_filters]`,
/// `#[use_interceptors]`, `Scoped<T>` parameters, per-argument pipes inside
/// `Parameters<…>`, and response masking. The advertised capabilities are
/// **derived** from the roles present, so a host cannot route what it forgot to
/// declare. `#[meta(EXPR)]` (repeatable) on an operation declares a value for
/// it, read over the host's of the same type. The impl is concrete: each
/// operation's site is a `static`, which no parameter of a generic impl
/// reaches.
///
/// An operation answers a `Result`, read by its type: a renamed `Result` or an
/// alias answers as the spelled one, over any error, and any other return is
/// refused at its type. A few readings go by the spelling, since no type tells
/// a macro them:
///
/// - a parameter is request-scoped when its type's last segment is `Scoped`,
///   and piped when it is `Parameters<Valid<T>>` or `Parameters<Piped<P, T>>`;
/// - a masked operation spells its return `Result<…>`, the mask reading
///   `Json<T>` to unwrap and `CallToolResult` to refuse off that spelling;
/// - rmcp derives a tool's output schema from a return spelled
///   `Result<Json<T>, …>`, and from none behind an alias.
///
/// A host that hand-writes `ServerHandler` (resources, completion) writes rmcp
/// directly and has no `#[tools]` block at all — `#[tools]` on a trait impl is
/// a compile error saying so.
///
/// # Expands to
///
/// The authored impl re-emitted **untouched**, each operation's `Handler` and
/// `static` site, plus — inside a private child module that carries rmcp's
/// imports, so the host file needs none — a delegating wrapper per operation
/// carrying `#[tool(name = "…")]` (the authored name stays the wire name),
/// rmcp's routers with `pub(crate)` visibility, and the `ServerHandler` impl
/// with its `get_info`. Each wrapper dispatches its site — the guards, the
/// posture's gate, the exception filters, the interceptors — its tail running
/// the pipes, the `Scoped<T>` parameters, the authored method and the response
/// mask, in that order.
#[proc_macro_attribute]
pub fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(mcp::tools(args, input).into()).into()
}
