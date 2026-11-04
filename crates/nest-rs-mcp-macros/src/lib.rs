//! `#[mcp]` / `#[tools]` decorators, re-exported by `nest-rs-mcp`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod mcp;
mod mcp_impl;
mod resource;
mod uri_template;

/// Behaves like `#[injectable]` for construction and emits a `Discoverable`
/// that attaches an `HttpEndpointMeta`. Its operations go under
/// [`macro@tools`] on the host's inherent impl; a host serving what no
/// decorator declares — completion, logging levels, subscriptions — writes
/// rmcp's `ServerHandler` by hand instead. The factory runs per session, so
/// per-session state stays fresh.
///
/// Every argument is optional. `path` is the **whole URL path** — omit it to
/// serve `nest_rs_mcp::DEFAULT_PATH` (`/mcp`). Nothing nests under it: it names
/// the one endpoint the host joins, and peers that write the same one share it.
/// `name` / `title` override the app's `McpIdentity` per field for that
/// endpoint; two hosts on one path both declaring fails boot. `version` and
/// `instructions` are refused: they are declared once, on the app's
/// `McpModule::for_root(McpIdentity::…)`.
/// A `#[meta(EXPR)]` below it is declared for every operation of the host. The
/// struct is concrete: each operation's site is a `static`, which no parameter
/// of a generic struct reaches.
///
/// `#[use_guards]`, `#[use_interceptors]`, `#[use_exception_filters]` and
/// `#[meta]` below it reach the operations a [`macro@tools`] impl declares, so
/// a host writing its `ServerHandler` by hand — which declares none — binding
/// one does not compile: a guard there would gate nothing.
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

/// The operations half of an MCP host: its `#[tool]`, `#[prompt]`,
/// `#[resource]` and `#[resources]` methods.
///
/// Takes no arguments — path and identity are `#[mcp]`'s, on the struct.
///
/// It absorbs rmcp's three-block shape (`#[tool_router]`, `#[prompt_router]`,
/// `#[tool_handler]`/`#[prompt_handler]` + `get_info`) into generated code,
/// writes the resource methods rmcp has no router for, and gives each
/// operation the request layers every other edge has:
/// `#[use_guards]` / `#[force_guards]`, a posture
/// (`#[authorize(Action, Entity)]` or `#[public]`, which the boot holds against who
/// reaches the endpoint), `#[use_exception_filters]`,
/// `#[use_interceptors]`, `Scoped<T>` and `Ambient<T>` parameters, per-argument pipes inside
/// `Parameters<…>`, and response masking. The advertised capabilities are
/// **derived** from the roles present, so a host cannot route what it forgot to
/// declare. `#[meta(EXPR)]` (repeatable) on an operation declares a value for
/// it, read over the host's of the same type. The impl is concrete: each
/// operation's site is a `static`, which no parameter of a generic impl
/// reaches.
///
/// `#[resource(template = "post://{id}")]` reads one resource: the template is
/// RFC 6570 level 1, each `{variable}` a parameter of the method parsed with
/// `FromStr`, and `resources/templates/list` advertises it with its `name`
/// (the method's unless stated), `title`, `description` and the media type its
/// answer is read as — `Json<T>` as `application/json`, a `String` as
/// `text/plain`, a `ReadResourceResult` as built, any of them inside `Option`,
/// `None` answering as a URI no template reads. `#[resources]` lists what can
/// be read, `Vec<Resource>`, one method per host. A URI is offered to the
/// templates in declaration order; one no template reads, whose variables do
/// not parse, or whose method answers `None` is MCP's *Resource not found*,
/// quoting nothing.
///
/// An operation answers a `Result`, read by its type: a renamed `Result` or an
/// alias answers as the spelled one, over any error, and any other return is
/// refused at its type. A few readings go by the spelling, since no type tells
/// a macro them:
///
/// - a parameter is request-scoped when its type's last segment is `Scoped`,
///   what a guard attached when it is `Ambient` (`nest_rs_core::Ambient`) — each
///   resolved in the tail through a function returning its own type, so a
///   namesake does not compile, and neither an argument of the protocol —
///   piped when it is `Parameters<Piped<P>>`, and on a `#[resource]` the
///   template variable its name spells — each argument held to the rules its
///   type declares;
/// - a masked operation spells its return `Result<…>`, the mask reading
///   `Json<T>` or `Option<Json<T>>` to unwrap and a tool's opaque answers
///   (`CallToolResult`, `ContentBlock`, …) or a `ReadResourceResult`, bare or
///   in its `Option`, to refuse off that spelling; a `#[prompt]`'s messages
///   and a `#[resources]` listing's entries are never masked, so a gated one
///   declares `unmasked`;
/// - rmcp derives a tool's output schema from a return spelled
///   `Result<Json<T>, …>`, and from none behind an alias.
///
/// A host that hand-writes `ServerHandler` (completion, logging levels,
/// subscriptions) writes rmcp directly and has no `#[tools]` block at all —
/// `#[tools]` on a trait impl is a compile error saying so.
///
/// # Expands to
///
/// The authored impl re-emitted **untouched**, each operation's `Handler` and
/// `static` site, each template's split parts, plus — inside a private child
/// module that carries rmcp's imports, so the host file needs none — a
/// delegating wrapper per operation, a tool's or a prompt's carrying
/// `#[tool(name = "…")]` (the authored name stays the wire name), rmcp's
/// routers with `pub(crate)` visibility, and the `ServerHandler` impl with its
/// `get_info` and, for the resource roles, `list_resources`,
/// `list_resource_templates` and `read_resource`. Each wrapper dispatches its
/// site — the guards, the posture's gate, the exception filters, the
/// interceptors — its tail running the pipes, the `Scoped<T>` and `Ambient<T>` parameters, the
/// authored method and the response mask, in that order.
#[proc_macro_attribute]
pub fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(mcp::tools(args, input).into()).into()
}
