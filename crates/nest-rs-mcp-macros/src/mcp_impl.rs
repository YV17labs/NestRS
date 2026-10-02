//! `#[mcp]` on an `impl` block — the operations half of the decorator.
//!
//! # What it absorbs
//!
//! rmcp's architecture asks a host for three blocks: `#[tool_router] impl T`,
//! `#[prompt_router] impl T`, and `#[tool_handler] #[prompt_handler] impl
//! ServerHandler for T` with a `get_info` declaring capabilities. Every other
//! edge in this framework is *one* decorated `impl` with decorated methods
//! (`#[routes]`, `#[resolver]`, `#[processor]`, `#[scheduled]`), so MCP was the
//! only one leaking its SDK's shape into the file a developer writes.
//!
//! This expansion takes one authored `impl` and emits all of it: the methods are
//! split by role into rmcp's two routers, the `ServerHandler` impl is generated
//! with the handler attributes it needs, and its capabilities are **derived**
//! from the roles actually present — a host can no longer route tools it forgot
//! to advertise.
//!
//! # The request layers, same as every other edge
//!
//! An operation declares the same things a `#[query]` does, and they expand to
//! the same four steps, in this order:
//!
//! 1. **Guards** — `#[use_guards(...)]` on the host struct and beside the
//!    operation, composed once per site and deduped against the app-wide pool
//!    (`run_layered_mcp_chain`).
//! 2. **The access posture** — `#[authorize(Action, Entity)]` emits the
//!    class-level gate; `#[public]` declares there is none. Exactly one is
//!    **required**: an operation nobody thought about must not compile, rather
//!    than ship ungated and unmasked.
//! 3. **Pipes** — a `Parameters<Valid<T>>` / `Parameters<Piped<P, T>>` argument
//!    exposes `T` on the wire (so the tool's JSON Schema is `T`'s) and runs the
//!    pipe before the body. A rejection is `invalid_params`, which is the one
//!    MCP error a model can act on.
//! 4. **Response masking** — `#[authorize]` also masks the returned value
//!    through the caller's ability, exactly as `#[resolver]` and `#[routes]` do.
//!
//! # Why a delegating wrapper rather than a rewritten body
//!
//! The authored method is re-emitted **untouched** in a plain inherent impl, and
//! the `#[tool]` attribute goes on a generated wrapper beside it that runs the
//! four steps and then calls it. Two things fall out of that which a rewritten
//! body would not give:
//!
//! * the developer's method keeps its real signature — `Valid<T>` where the
//!   author wrote `Valid<T>` — so a unit test calls it directly and a compile
//!   error about the body points at the body;
//! * the wire signature is free to differ from it, which is exactly what a pipe
//!   needs (`T` on the wire, the carrier in the body).
//!
//! The wrapper carries `#[tool(name = "…")]` so the *authored* name is still the
//! one the protocol addresses — the generated ident never reaches the wire.
//!
//! # Why a private child module
//!
//! rmcp's macros expand to bare `rmcp::` paths resolved against the call site,
//! which is why a host file had to carry `use nest_rs::mcp::rmcp;` — an import
//! whose only job was someone else's hygiene, and which the CLI template shipped
//! with three lines of comment explaining it. Emitting the impls inside a
//! private module lets *that* module carry the imports instead.
//!
//! The Rust fact that makes it sound is pinned by
//! `tests/integration/mcp_impl.rs`: an inherent impl may be written in any
//! module of the defining crate, and a descendant still reaches the parent's
//! private fields.
//!
//! Visibility is the wrinkle. rmcp generates `tool_router()` without `pub`, so
//! the router would die at this module's edge and the duplicate-tool boot check
//! — which reads it from the parent — would silently see an empty list. rmcp
//! answers that itself: `#[tool_router(vis = "…")]` sets the generated
//! function's visibility, so asking for `pub(crate)` is all this needs. An
//! earlier draft grew a second accessor and a second fallback trait to work
//! around a problem the SDK had already solved.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Path, Signature, Token, Type,
};

use nest_rs_codegen::{
    Collision, Conditional, DispatchKeys, HostBorrow, PipeWrapper, Posture, PostureRules,
    await_if_async, cfg_attrs, force_guard_typeids, forwarded_arg_idents, generic_args,
    guard_capability_bounds, impl_self_ident, layer_deps, normalize_forwarded_args,
    nth_generic_type, pipe_wrapper, reject_http_only_layers, scoped_specs, shared_receiver,
    snake_case, take_path_list, type_label,
};

/// The decorated methods of one authored `impl`, already partitioned by the
/// router each belongs to. Holding the split once is what lets every downstream
/// question — which routers to emit, which handler attributes, which
/// capabilities — be a plain `is_empty()`.
#[derive(Default)]
struct Operations {
    tools: Vec<Operation>,
    prompts: Vec<Operation>,
}

impl Operations {
    fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.prompts.is_empty()
    }

    fn all(&self) -> impl Iterator<Item = &Operation> {
        self.tools.iter().chain(self.prompts.iter())
    }
}

/// Which router a decorated method belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Tool,
    Prompt,
}

impl Role {
    /// Every role, paired with the bare attribute name that declares it.
    ///
    /// **One table, three readers** — the predicate, the diagnostic that quotes
    /// the attribute back, and the one-role refusal's accepted set. It shipped
    /// as a fourth copy beside three hand-spelled `is_ident("tool")` /
    /// `"#[tool]"` matches, with a doc claiming the coupling that would have
    /// prevented them: a constant nothing reads is a constant that drifts, and
    /// one whose doc asserts it is read is worse, because the next reader
    /// believes the check already exists.
    /// Both spellings sit on one line per role — the bare name a path matches
    /// and the bracketed form a diagnostic quotes — so the two cannot drift
    /// apart the way three separate `match` arms did.
    const ALL: [(Self, &'static str, &'static str); 2] = [
        (Self::Tool, "tool", "#[tool]"),
        (Self::Prompt, "prompt", "#[prompt]"),
    ];

    /// The bare attribute names, for the refusal that lists what is accepted.
    fn names() -> [&'static str; 2] {
        [Self::ALL[0].1, Self::ALL[1].1]
    }

    /// The role an attribute gives a method, if it gives one.
    fn from_attr(attr: &Attribute) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, name, _)| attr.path().is_ident(name))
            .map(|(role, _, _)| *role)
    }

    /// The attribute a reader wrote, for a diagnostic that quotes it back.
    fn attr(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(role, _, _)| *role == self)
            .map(|(_, _, bracketed)| *bracketed)
            .unwrap_or_default()
    }

    /// The `McpOperationKind` variant this role reports to a guard.
    fn kind(self) -> TokenStream2 {
        match self {
            Self::Tool => quote!(::nest_rs_mcp::McpOperationKind::Tool),
            Self::Prompt => quote!(::nest_rs_mcp::McpOperationKind::Prompt),
        }
    }

    /// The word a chain label and a log field spell this role with.
    fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Prompt => "prompt",
        }
    }
}

/// One decorated operation: what the developer wrote, and everything the wrapper
/// has to know to run the four steps around it.
struct Operation {
    role: Role,
    /// The role attribute as it will sit on the wrapper — description filled in
    /// from the doc comment, `name` pinned to the authored method's own.
    role_attr: TokenStream2,
    /// A `const` refusing a blank description the expansion could only read as
    /// an expression — `None` when it was a literal, already checked here.
    description_check: Option<TokenStream2>,
    /// The wrapper's own ident — never on the wire.
    wrapper: syn::Ident,
    /// The authored signature, with every argument pattern normalized to the
    /// plain identifier it binds.
    sig: Signature,
    guards: Vec<Path>,
    force_guards: Vec<Path>,
    posture: Posture,
    pipes: Vec<PipedArg>,
    /// The authored method's `#[cfg]` conditions, which its router group, and so
    /// its wrapper and its route, are compiled under.
    cfgs: Vec<TokenStream2>,
}

impl Operation {
    /// The authored method's name: what the protocol addresses, and what the
    /// wrapper delegates to. Read off the signature rather than stored beside
    /// it, so the two cannot disagree.
    fn name(&self) -> &syn::Ident {
        &self.sig.ident
    }
}

/// A `#[tool]` / `#[prompt]` parameter whose wire value goes through a pipe:
/// `Parameters<Valid<T>>` or `Parameters<Piped<P, T>>`.
struct PipedArg {
    ident: syn::Ident,
    /// The pipe `P` in `Piped<P, T>`; `None` for `Valid<T>` (validation).
    pipe: Option<Path>,
    /// `T` — what the wire carries and what rmcp derives the schema from.
    value_ty: Type,
}

pub(crate) fn mcp_impl(args: TokenStream, item: ItemImpl) -> TokenStream {
    if let Err(err) = crate::mcp::MCP_PAIR.reject_args(
        &TokenStream2::from(args),
        "the endpoint's path and identity are declared by",
    ) {
        return err.to_compile_error().into();
    }
    match expand(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand(mut item: ItemImpl) -> syn::Result<TokenStream2> {
    // The trait-impl refusal used to be worded here, and it was the only one of
    // the nine impl halves that had an answer at all. It is now
    // `DecoratorPair::parse_operations`, so every half says it and says it the
    // same way; a hand-written `impl ServerHandler` (the escape hatch a host
    // with no `#[tools]` block takes) is what the shared sentence redirects to.
    reject_http_only_layers(&item.attrs, "MCP", "host")?;
    crate::mcp::MCP_PAIR.reject_host_layers(&item.attrs)?;

    let self_ty = item.self_ty.clone();
    // **`#[tools]`, because this file is `#[tools]`' expansion.** The pair
    // split exists so "the compiler can tell the reader which decorator it is
    // looking at" (`CLAUDE.md`, *No decorator on two item shapes*); three
    // sentences here named the decorator on the *struct* and gave that back by
    // hand.
    let base = impl_self_ident(&self_ty, "#[tools]")?;
    let (operations, declared) = take_operations(&mut item, &base)?;
    let markers = declared.markers(&self_ty, &item.generics);
    if operations.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[tools] on an impl with no #[tool] or #[prompt] method has nothing to \
             mount — drop the decorator, or mark the methods it should serve",
        ));
    }

    let module = format_ident!("__nest_rs_mcp_{}", snake_case(&base.to_string()),);
    let generics = item.generics.split_for_impl();

    // `pub(crate)` is what carries the router out of this module, so the boot
    // checks can still read the host's tool names.
    let tool_impl = router_impl(
        &self_ty,
        &generics,
        &operations.tools,
        &Router {
            attr: quote!(tool_router),
            name: format_ident!("tool_router"),
            ty: quote!(rmcp::handler::server::router::tool::ToolRouter),
            vis: quote!(pub(crate)),
        },
    )?;
    let prompt_impl = router_impl(
        &self_ty,
        &generics,
        &operations.prompts,
        &Router {
            attr: quote!(prompt_router),
            name: format_ident!("prompt_router"),
            ty: quote!(rmcp::handler::server::router::prompt::PromptRouter),
            vis: quote!(),
        },
    )?;

    let handler_attrs = {
        let tools = (!operations.tools.is_empty()).then(|| quote!(#[tool_handler]));
        let prompts = (!operations.prompts.is_empty()).then(|| quote!(#[prompt_handler]));
        quote!(#tools #prompts)
    };

    // Capabilities are *derived*, never restated: a method in a router is the
    // proof the surface exists, so a host cannot route tools it forgot to
    // advertise. Derived from what survives `#[cfg]`, not from what was written:
    // each operation switches its surface on under its own conditions, so a host
    // whose every tool is compiled out claims no tools surface. A hand-written
    // surface (resources, completion) is declared by hand in its own
    // `impl ServerHandler`, which is the one shape this does not generate.
    let capabilities = {
        let surface = |ops: &[Operation], field: TokenStream2| {
            ops.iter()
                .map(|op| {
                    let cfgs = &op.cfgs;
                    // A `let`, because a condition may sit on a statement of that
                    // kind and not on an assignment.
                    quote! {
                        #(#cfgs)*
                        let () = __capabilities.#field = ::core::option::Option::Some(
                            ::core::default::Default::default(),
                        );
                    }
                })
                .collect::<Vec<_>>()
        };
        let tools = surface(&operations.tools, quote!(tools));
        let prompts = surface(&operations.prompts, quote!(prompts));
        quote! {{
            #[allow(unused_mut)]
            let mut __capabilities = ServerCapabilities::builder().build();
            #(#tools)*
            #(#prompts)*
            __capabilities
        }}
    };

    // Every guard any operation declared, deduped, for `Discoverable::injected`
    // — the struct half reads this back through `DefaultOperationLayers`, so a
    // guard no reachable module provides fails the boot naming it rather than
    // resolving to nothing at the first call.
    let operation_guards = || {
        operations.all().flat_map(|op| {
            op.guards
                .iter()
                .chain(op.force_guards.iter())
                .map(|item| Conditional {
                    cfgs: &op.cfgs,
                    item,
                })
        })
    };
    // Operation-scope guards run `Guard::check_mcp`, whose default is `Ok(())`.
    let capability_bounds =
        guard_capability_bounds(operation_guards(), quote!(::nest_rs_guards::McpGuard));
    let layers = layer_deps(operation_guards());
    // Each under its operation's own conditions: a compiled-out operation sends
    // no description, so it owes none.
    let description_checks = operations.all().filter_map(|op| {
        let cfgs = &op.cfgs;
        op.description_check
            .as_ref()
            .map(|check| quote!(#(#cfgs)* #check))
    });
    let layer_keys = &layers.keys;
    let layer_labels = &layers.labels;

    let (impl_generics, ty_generics, where_clause) = &generics;
    Ok(quote! {
        #item

        #capability_bounds

        #(#description_checks)*

        #markers

        impl #impl_generics #self_ty #ty_generics #where_clause {
            #[doc(hidden)]
            pub fn __nestrs_mcp_operation_layers()
                -> (::std::vec::Vec<::core::any::TypeId>, ::std::vec::Vec<&'static str>)
            {
                (::std::vec![#(#layer_keys),*], ::std::vec![#(#layer_labels),*])
            }
        }

        #[doc(hidden)]
        mod #module {
            use super::*;

            // The names rmcp's own expansions resolve against — the import that
            // used to sit in the developer's file, now scoped to generated code.
            use ::nest_rs_mcp::rmcp;
            use ::nest_rs_mcp::{ServerCapabilities, ServerConfig};
            use ::nest_rs_mcp::{
                ServerHandler, prompt, prompt_handler, prompt_router, tool, tool_handler,
                tool_router,
            };

            #tool_impl
            #prompt_impl

            #handler_attrs
            impl #impl_generics ServerHandler for #self_ty #ty_generics #where_clause {
                fn get_info(&self) -> ServerConfig {
                    ServerConfig::new(#capabilities)
                }
            }
        }
    })
}

/// One of rmcp's two routers, as `router_impl` emits it.
struct Router {
    /// rmcp's attribute that builds a router out of an impl's methods.
    attr: TokenStream2,
    /// The router function rmcp's handler attribute reads.
    name: syn::Ident,
    /// The router type, as the generated module's `rmcp` import names it.
    ty: TokenStream2,
    vis: TokenStream2,
}

/// The rmcp router carrying `ops`' wrappers, or nothing when the host serves none
/// of that role.
///
/// **One rmcp router impl per set of `#[cfg]` conditions**, merged into the one
/// router function the handler reads. rmcp's router attribute routes every
/// method it finds whatever the method's `#[cfg]`, so a compiled-out operation
/// left the route naming a function that no longer exists. A condition on the
/// *impl* removes the group's methods and its routes together, before rmcp's
/// attribute ever runs, and the merge that adds it sits under the same
/// condition.
fn router_impl(
    self_ty: &Type,
    (impl_generics, ty_generics, where_clause): &Generics<'_>,
    ops: &[Operation],
    router: &Router,
) -> syn::Result<Option<TokenStream2>> {
    if ops.is_empty() {
        return Ok(None);
    }
    let mut groups: Vec<(&[TokenStream2], Vec<TokenStream2>)> = Vec::new();
    for op in ops {
        let wrapped = wrapper(self_ty, op)?;
        let own = &op.cfgs;
        let conditions = quote!(#(#own)*).to_string();
        match groups
            .iter_mut()
            .find(|(cfgs, _)| quote!(#(#cfgs)*).to_string() == conditions)
        {
            Some((_, methods)) => methods.push(wrapped),
            None => groups.push((&op.cfgs, vec![wrapped])),
        }
    }
    let Router {
        attr,
        name,
        ty,
        vis,
    } = router;
    let impls = groups.iter().enumerate().map(|(index, (cfgs, methods))| {
        // A string, which both of rmcp's router attributes read the name from.
        let group = LitStr::new(&format!("__nestrs_{name}_{index}"), name.span());
        quote! {
            #(#cfgs)*
            #[#attr(router = #group)]
            impl #impl_generics #self_ty #ty_generics #where_clause {
                #(#methods)*
            }
        }
    });
    let merges = groups.iter().enumerate().map(|(index, (cfgs, _))| {
        let group = format_ident!("__nestrs_{}_{}", name, index);
        quote! {
            #(#cfgs)*
            __router.merge(Self::#group());
        }
    });
    Ok(Some(quote! {
        #(#impls)*

        impl #impl_generics #self_ty #ty_generics #where_clause {
            #vis fn #name() -> #ty<Self> {
                #[allow(unused_mut)]
                let mut __router = #ty::<Self>::new();
                #(#merges)*
                __router
            }
        }
    }))
}

/// The three halves of `Generics::split_for_impl`, computed once by `expand` and
/// handed down rather than re-derived per router.
type Generics<'a> = (
    syn::ImplGenerics<'a>,
    syn::TypeGenerics<'a>,
    Option<&'a syn::WhereClause>,
);

/// The generated method that rmcp routes to: the four request layers, then the
/// authored method.
fn wrapper(self_ty: &Type, op: &Operation) -> syn::Result<TokenStream2> {
    let Operation {
        role,
        role_attr,
        wrapper,
        sig,
        posture,
        pipes,
        ..
    } = op;
    let name = op.name();

    // The wire signature: the authored one with each pipe carrier replaced by
    // the value it wraps, so rmcp derives the tool's JSON Schema from `T` rather
    // than from a carrier it cannot see through.
    let mut wire_sig = sig.clone();
    wire_sig.ident = wrapper.clone();
    for input in wire_sig.inputs.iter_mut() {
        let FnArg::Typed(pat_type) = input else {
            continue;
        };
        let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
            continue;
        };
        if let Some(arg) = pipes.iter().find(|arg| arg.ident == pat_ident.ident) {
            let value = &arg.value_ty;
            *pat_type.ty = syn::parse_quote!(::nest_rs_mcp::Parameters<#value>);
        }
    }

    let label = LitStr::new(
        &format!("{} {}::{}", role.label(), type_label(self_ty), name),
        name.span(),
    );
    let chain = guard_chain(self_ty, op, &label);
    let gate = access_gate(posture);
    let pipe_prelude = pipes.iter().map(pipe_statement);
    // `take_operation` normalized every pattern to the ident it binds, so this
    // is the same forwarding list `#[resolver]` builds — from the same helper,
    // which is also where the "binds no name / binds several" diagnostics live.
    let call_args = forwarded_arg_idents(sig)?;
    let call = await_if_async(sig, quote!(<#self_ty>::#name(self, #(#call_args),*)));
    let body = mask(posture, sig, call)?;
    // The wrapper awaits its guard chain whatever the authored method is, so it
    // is `async` even where the method it delegates to is not.
    wire_sig.asyncness = Some(syn::token::Async(name.span()));

    // Every step that can refuse runs inside one block answering
    // `Result<_, McpError>`, and the wrapper turns a refusal into the
    // operation's own answer at **one** site, typed by `OperationAnswer`: a
    // `Result` whose error takes an `McpError` is one whatever it is called,
    // and anything else is refused there once, at the return type — not by a
    // `?` per step deep inside the expansion. What the block hands back are the
    // arguments the pipes rebound.
    let piped: Vec<&syn::Ident> = pipes.iter().map(|arg| &arg.ident).collect();
    let span = match &sig.output {
        syn::ReturnType::Type(_, ty) => ty.span(),
        syn::ReturnType::Default => sig.ident.span(),
    };
    let refused = quote_spanned! {span=> ::nest_rs_mcp::refused(__nestrs_refusal) };
    Ok(quote! {
        #role_attr
        #wire_sig {
            let __nestrs_ready: ::core::result::Result<_, ::nest_rs_mcp::McpError> = async {
                #chain
                #gate
                #(#pipe_prelude)*
                ::core::result::Result::Ok((#(#piped,)*))
            }
            .await;
            let (#(#piped,)*) = match __nestrs_ready {
                ::core::result::Result::Ok(__nestrs_ready) => __nestrs_ready,
                ::core::result::Result::Err(__nestrs_refusal) => return #refused,
            };
            #body
        }
    })
}

/// Step 1 — the host-scope + operation-scope guard chain.
///
/// Always emitted, even when the operation declares none: the *host* may have,
/// and the impl half cannot see the struct's attributes. Composition is memoized
/// per app, so the steady-state cost of a host with no guards at all is one
/// atomic load over an empty slice.
///
/// One call, not an inlined preamble — the ambient lookups, the context and the
/// no-app fallback live in `run_layered_mcp_chain` so they are codegen'd once
/// per crate rather than once per operation.
fn guard_chain(self_ty: &Type, op: &Operation, label: &LitStr) -> TokenStream2 {
    let method_specs = scoped_specs(&op.guards, quote!(dyn ::nest_rs_guards::Guard));
    let force = force_guard_typeids(&op.force_guards);
    let kind = op.role.kind();
    let name = LitStr::new(&op.name().to_string(), op.name().span());
    quote! {
        {
            // Composed once per site against this app, then memoized — the MCP
            // analog of `RouteShaper`'s mount-time composition.
            static __NESTRS_GUARD_CHAIN: ::nest_rs_guards::SiteChainCell =
                ::nest_rs_guards::SiteChainCell::new();
            ::nest_rs_guards::run_layered_mcp_chain(
                &__NESTRS_GUARD_CHAIN,
                #label,
                ::core::any::type_name::<#self_ty>(),
                #kind,
                #name,
                &|| ::nest_rs_guards::SiteChainSources {
                    provider: <#self_ty>::__nestrs_mcp_host_guard_specs(),
                    method: #method_specs,
                    force: #force,
                },
            ).await?;
        }
    }
}

/// Step 2 — the class-level gate `#[authorize(Action, Entity)]` desugars to.
fn access_gate(posture: &Posture) -> Option<TokenStream2> {
    match posture {
        Posture::Authorize { action, entity, .. } => Some(quote! {
            ::nest_rs_authz::mcp::authorize::<#action, #entity>()?;
        }),
        Posture::Public => None,
    }
}

/// Step 3 — run one argument's pipe over its wire value and rebind the parameter
/// to the carrier the body expects.
///
/// Runs *after* the gate, so a caller the class gate refuses never pays for
/// validation, and a validation message never doubles as an existence oracle.
fn pipe_statement(arg: &PipedArg) -> TokenStream2 {
    let PipedArg {
        ident,
        pipe,
        value_ty,
    } = arg;
    let apply = match pipe {
        Some(pipe) => quote!(::nest_rs_pipes::Piped::<#pipe, #value_ty>::apply(__nestrs_value)),
        None => quote!(::nest_rs_pipes::Valid::<#value_ty>::apply(__nestrs_value)),
    };
    quote! {
        let ::nest_rs_mcp::Parameters(__nestrs_value) = #ident;
        let #ident = ::nest_rs_mcp::Parameters(
            #apply.map_err(|__nestrs_err| ::nest_rs_mcp::pipe_error(&__nestrs_err))?,
        );
    }
}

/// Step 4 — the call, with response masking when the posture arms it.
///
/// `Json<T>` is unwrapped before masking and rewrapped after: rmcp's wrapper is
/// neither `Serialize` nor `DeserializeOwned` (it only delegates `JsonSchema`),
/// and `T` is the value that actually reaches the wire as `structuredContent`.
fn mask(posture: &Posture, sig: &Signature, call: TokenStream2) -> syn::Result<TokenStream2> {
    let Posture::Authorize {
        action,
        entity,
        unmasked,
    } = posture
    else {
        return Ok(call);
    };
    if *unmasked {
        return Ok(call);
    }

    // `take_operation` already refused a masked operation whose return is not
    // spelled `Result<…>`, so this is the shape by construction rather than a
    // second check with its own message.
    let Some(ok_ty) = result_ok_type(sig) else {
        return Ok(call);
    };
    // `CallToolResult` is opaque content — arbitrary blocks, not an entity shape
    // — so the value-level round-trip has nothing to reconcile it against. Say so
    // here rather than let it surface as an unmet `DeserializeOwned` bound deep
    // inside the expansion.
    if let Type::Path(path) = ok_ty
        && let Some(last) = path.path.segments.last()
        && matches!(
            last.ident.to_string().as_str(),
            "CallToolResult" | "CallToolResponse"
        )
    {
        return Err(syn::Error::new_spanned(
            &sig.output,
            "`#[authorize(...)]` cannot mask a `CallToolResult` — its content blocks are \
             opaque to the entity round-trip. Return the wire type (`Json<T>` or `T`) and \
             let the mask run, or keep the gate and mask by hand with \
             `#[authorize(Action, Entity, unmasked)]` + `nest_rs_authz::masked_reply`",
        ));
    }

    // `Json<T>`: mask `T`, then put the wrapper back.
    if let Some(inner) = nth_generic_type(ok_ty, "Json", 0) {
        return Ok(quote! {
            {
                let ::nest_rs_mcp::Json(__nestrs_out) = #call?;
                ::core::result::Result::Ok(::nest_rs_mcp::Json(
                    ::nest_rs_authz::mcp::masked_value_for::<#action, #entity, #inner>(
                        __nestrs_out,
                    )?,
                ))
            }
        });
    }

    Ok(quote! {
        {
            let __nestrs_out = #call?;
            // The mask's own `Result` *is* the operation's — no rewrap, and no
            // `Ok(..?)` for clippy to flag at every use site.
            ::nest_rs_authz::mcp::masked_value_for::<#action, #entity, _>(__nestrs_out)
        }
    })
}

/// The `T` of a `-> Result<T, E>` return, or `None` for anything else.
fn result_ok_type(sig: &Signature) -> Option<&Type> {
    let syn::ReturnType::Type(_, ty) = &sig.output else {
        return None;
    };
    nth_generic_type(ty, "Result", 0)
}

/// An attribute's bare identifier, for a refusal that names what was written.
/// Partition the decorated methods by role, taking each one's layer declarations
/// off the authored method as it goes.
///
/// The attributes are **consumed**: what stays on the re-emitted method is the
/// developer's own (`#[doc]`, `#[allow]`, …), and nothing the compiler would
/// reject as unknown.
fn take_operations(
    item: &mut ItemImpl,
    base: &syn::Ident,
) -> syn::Result<(Operations, DispatchKeys)> {
    let mut operations = Operations::default();
    // Each wire name routes to one method per router: rmcp's router keeps the
    // last route added under a name, so a second declaration replaced the first
    // in silence. Refused by the macro when neither carries a `#[cfg]`, by rustc
    // through the marker when both are compiled.
    let mut declared = DispatchKeys::new(
        "#[tools]",
        "a host routes each tool name, and each prompt name, to one method, so one of the \
         two would never run — give one a distinct `name`",
    );

    for entry in item.items.iter_mut() {
        let ImplItem::Fn(method) = entry else {
            return Err(unsupported(entry));
        };
        // **Every role attribute, not the first**, through the family's helper:
        // a method carrying both `#[tool]` and `#[prompt]` used to leave the
        // second on the re-emitted item for rmcp to route as an operation nobody
        // declared.
        let Some(index) =
            nest_rs_codegen::one_role_per_method("role", &method.attrs, &Role::names(), "")?
        else {
            // Helpers belong beside the struct: left here they would move into
            // the generated module, where a reader would not look for them.
            return Err(unsupported(entry));
        };
        let Some(role) = Role::from_attr(&method.attrs[index]) else {
            return Err(unsupported(entry));
        };
        shared_receiver(method, "#[tools]", base, HostBorrow::Host)?;
        nest_rs_codegen::concrete_signature(method, "#[tools]")?;

        let role_attr = &method.attrs[index];
        let wire_name = match stated_name(role_attr)? {
            Some(name) => name,
            None => method.sig.ident.to_string(),
        };
        declared.declare(
            Collision::Marker,
            role.label(),
            &wire_name,
            &format!(
                "{}(name = {wire_name:?})]",
                role.attr().trim_end_matches(']')
            ),
            &method.sig.ident,
            &cfg_attrs(&method.attrs),
            role_attr,
        )?;

        let operation = take_operation(method, index, role, base)?;
        match role {
            Role::Tool => operations.tools.push(operation),
            Role::Prompt => operations.prompts.push(operation),
        }
    }

    Ok((operations, declared))
}

/// The `name = "…"` stated inside `#[tool(...)]` / `#[prompt(...)]`, when one is.
fn stated_name(attr: &Attribute) -> syn::Result<Option<String>> {
    let Meta::List(_) = &attr.meta else {
        return Ok(None);
    };
    let args = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    Ok(args.iter().find_map(|meta| match meta {
        Meta::NameValue(value) if value.path.is_ident("name") => match &value.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(name),
                ..
            }) => Some(name.value()),
            _ => None,
        },
        _ => None,
    }))
}

/// Everything one decorated method declares, taken off it.
fn take_operation(
    method: &mut ImplItemFn,
    index: usize,
    role: Role,
    base: &syn::Ident,
) -> syn::Result<Operation> {
    let name = method.sig.ident.clone();
    let (role_attr, description_check) = wrapper_role_attr(method, index, role)?;
    // Removed after `wrapper_role_attr` has read the doc comment off it: the
    // attribute belongs to the wrapper now, and leaving a copy behind would make
    // rmcp route the authored method as a second tool of the same name.
    method.attrs.remove(index);

    reject_http_only_layers(&method.attrs, "MCP", "operation")?;
    // The guard chain and the access gate both refuse by returning, so the
    // operation must answer a `Result` — checked by **type**, at the one place
    // the wrapper refuses (`nest_rs_mcp::refused`), so a `Result` renamed on
    // import or behind an alias is one, and anything else is told so once.
    let guards = take_path_list(&mut method.attrs, "use_guards")?;
    let force_guards = take_path_list(&mut method.attrs, "force_guards")?;
    let posture = posture_rules(role).take(method)?;
    // The mask is the one step that reads the answer's *shape* — `Json<T>` to
    // unwrap, `CallToolResult` to refuse — and a shape inside a `Result` is
    // visible only through its spelling.
    if posture.masks() && result_ok_type(&method.sig).is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            format!(
                "a masked {} spells its return `Result<…, McpError>`: the mask reads the \
                 value's shape — `Json<T>` to unwrap, `CallToolResult` to refuse — off \
                 that spelling, and a `Result` renamed on import or behind an alias hides \
                 it. Write `Result<…>`, or declare `#[authorize(Action, Entity, unmasked)]` \
                 and mask in the body",
                role.attr(),
            ),
        ));
    }

    // The developer's method keeps its own patterns — this normalizes a clone, so
    // the wrapper has one plain ident per argument to declare and forward by.
    let mut sig = method.sig.clone();
    normalize_forwarded_args(sig.inputs.iter_mut())?;
    let pipes = piped_args(&sig)?;

    // The method's conditions reach every item emitted for it — its wrapper, its
    // route, its guards. Nothing else does: a doc comment is the authored
    // method's prose for a reader (the wrapper carries the model's copy as
    // `description`), and an `#[allow]` governs a body the wrapper does not hold.
    let cfgs = cfg_attrs(&method.attrs);

    Ok(Operation {
        role,
        role_attr,
        description_check,
        wrapper: format_ident!("__nestrs_mcp_{}_{}", snake_case(&base.to_string()), name),
        sig,
        guards,
        force_guards,
        posture,
        pipes,
        cfgs,
    })
}

/// The sentence an operation with no description is refused with — at expansion
/// when the prose is a literal, and by the compiler when only a constant
/// evaluation can read it.
const NEEDS_A_DESCRIPTION: &str = "an MCP operation needs a description — the model reads it to \
                                   choose between operations. Write a doc comment above it, or \
                                   state `description = \"…\"` on the attribute";

/// The `#[tool]` / `#[prompt]` attribute as the wrapper will carry it: the
/// author's own arguments, plus the `name` the protocol must keep addressing
/// and — only when the attribute states none — a `description` lifted from the
/// doc comment. An operation with neither is a compile error: the description
/// is what a model reads to choose between operations, so it is not optional.
///
/// **A blank description is no description**, wherever it comes from, and the
/// family is every way the framework reads one:
///
/// - a doc comment of literals, or a stated `description = "…"`, is read here
///   and refused here when it trims to nothing;
/// - a doc line that is a macro (`#[doc = include_str!(…)]`, `concat!(…)`) and
///   a stated `description = <macro or constant>` are values only the compiler
///   evaluates, so the second element is a `const` refusing them with the same
///   sentence when they evaluate blank — an empty file was shipped to the model
///   as the whole description while `#[doc = "   "]` was refused;
/// - any other stated expression is computed at run time, where no
///   compile-time check can read it, and is rmcp's to send as written.
fn wrapper_role_attr(
    method: &ImplItemFn,
    index: usize,
    role: Role,
) -> syn::Result<(TokenStream2, Option<TokenStream2>)> {
    let attr = &method.attrs[index];
    let stated = stated_keys(attr)?;
    let role_path = attr.path().clone();

    let mut extra: Vec<TokenStream2> = Vec::new();
    let description = match stated_description(attr)? {
        Some(stated) => Some(stated),
        None => {
            // The attribute is the declared form — the prose is a value the
            // decorator compiles in, which is what lets a codebase that carries
            // no comments still describe its tools. The doc comment is the
            // fallback for one that does, so the sentence is never written
            // twice.
            let doc = doc_comment(&method.attrs);
            if let Some(doc) = &doc {
                extra.push(quote!(description = #doc));
            }
            doc.map(Description::read)
        }
    };
    let description_check = match description {
        None => {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                NEEDS_A_DESCRIPTION,
            ));
        }
        Some(Description::Read | Description::Runtime) => None,
        Some(Description::Evaluated(expr)) => {
            let span = method.sig.ident.span();
            Some(quote_spanned! {span=>
                const _: () = ::core::assert!(
                    !::nest_rs_mcp::description_is_blank(#expr),
                    #NEEDS_A_DESCRIPTION,
                );
            })
        }
    };
    if !stated.iter().any(|key| key == "name") {
        // The wrapper's ident is generated, so without this the wire name would
        // be an implementation detail. Pinning it is what keeps the delegation
        // invisible to a client.
        let name = method.sig.ident.to_string();
        extra.push(quote!(name = #name));
    }

    let rest = match &attr.meta {
        Meta::Path(_) => quote!(),
        Meta::List(list) => {
            let tokens = &list.tokens;
            quote!(#tokens,)
        }
        Meta::NameValue(value) => {
            return Err(syn::Error::new_spanned(
                value,
                format!("expected `{}` or `{}(...)`", role.attr(), role.attr()),
            ));
        }
    };

    // Left as tokens: its only consumer is a `quote!` interpolation, so parsing
    // it back into an `Attribute` would be a round trip with a fallible step in
    // the middle and nothing on the other side that needs the typed form.
    Ok((quote!(#[#role_path(#rest #(#extra),*)]), description_check))
}

/// A description, as far as the expansion can read it.
enum Description {
    /// A literal, read here, holding prose.
    Read,
    /// A macro of literals or a constant: a value only the compiler evaluates.
    Evaluated(TokenStream2),
    /// An expression computed at run time.
    Runtime,
}

impl Description {
    /// A doc comment as [`doc_comment`] emits it: one literal — never blank,
    /// [`doc_comment`] answers `None` for that — or a `concat!`.
    fn read(doc: TokenStream2) -> Self {
        match syn::parse2::<LitStr>(doc.clone()) {
            Ok(_) => Self::Read,
            Err(_) => Self::Evaluated(doc),
        }
    }
}

/// The `description = …` the author stated inside `#[tool(...)]`, read as far
/// as the compiler can read it. A blank literal is refused at the literal.
fn stated_description(attr: &Attribute) -> syn::Result<Option<Description>> {
    let Meta::List(_) = &attr.meta else {
        return Ok(None);
    };
    let args = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    let Some(value) = args.iter().find_map(|meta| match meta {
        Meta::NameValue(pair) if pair.path.is_ident("description") => Some(&pair.value),
        _ => None,
    }) else {
        return Ok(None);
    };
    Ok(Some(match nest_rs_codegen::ungrouped_expr(value) {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) => {
            if text.value().trim().is_empty() {
                return Err(syn::Error::new_spanned(text, NEEDS_A_DESCRIPTION));
            }
            Description::Read
        }
        expr @ (syn::Expr::Macro(_) | syn::Expr::Path(_)) => Description::Evaluated(quote!(#expr)),
        _ => Description::Runtime,
    }))
}

/// The top-level keys the author already stated inside `#[tool(...)]`.
///
/// Parsing the arguments as a `Meta` list is what keeps every other key rmcp
/// accepts intact: `Meta::List` captures a nested group like
/// `annotations(title = "…", read_only_hint = true)` as opaque tokens without
/// descending into it, so nothing here has to know rmcp's grammar.
fn stated_keys(attr: &Attribute) -> syn::Result<Vec<String>> {
    let Meta::List(_) = &attr.meta else {
        return Ok(Vec::new());
    };
    let args = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    Ok(args
        .iter()
        .filter_map(|meta| meta.path().get_ident().map(ToString::to_string))
        .collect())
}

/// The method's doc comment, joined into the one sentence rmcp sends — as an
/// expression, because rmcp takes one for `description`.
///
/// A literal when every line is one, which is the common case and the same
/// sentence as ever. **A `#[doc = include_str!(…)]` or `#[doc = concat!(…)]`
/// line is kept**, the whole sentence then emitted through `concat!`: it was
/// dropped for not being a string literal, so a model was sent half the prose
/// in silence — and a doc written only that way was refused as absent, telling
/// the developer to write the doc comment they had written.
fn doc_comment(attrs: &[Attribute]) -> Option<TokenStream2> {
    let lines: Vec<&syn::Expr> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(value) => Some(&value.value),
            _ => None,
        })
        .collect();
    let literal = |line: &syn::Expr| match nest_rs_codegen::ungrouped_expr(line) {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) => Some(text.value().trim().to_owned()),
        _ => None,
    };
    if lines.iter().all(|line| literal(line).is_some()) {
        let joined = lines
            .iter()
            .filter_map(|line| literal(line))
            .collect::<Vec<_>>()
            .join(" ");
        let trimmed = joined.trim();
        return (!trimmed.is_empty()).then(|| quote!(#trimmed));
    }
    let pieces: Vec<TokenStream2> = lines
        .iter()
        .filter_map(|line| match literal(line) {
            Some(text) if text.is_empty() => None,
            Some(text) => Some(quote!(#text)),
            None => Some(quote!(#line)),
        })
        .collect();
    let mut spaced = Vec::with_capacity(pieces.len() * 2);
    for (index, piece) in pieces.into_iter().enumerate() {
        if index > 0 {
            spaced.push(quote!(" "));
        }
        spaced.push(piece);
    }
    Some(quote!(::core::concat!(#(#spaced),*)))
}

/// MCP's half of the shared posture grammar, per operation role so the refusal
/// quotes back the attribute the developer actually wrote.
fn posture_rules(role: Role) -> PostureRules {
    PostureRules {
        operation: role.attr(),
        public_means: "no gate and no mask — `#[use_guards]` guards still run, and the \
                       endpoint's own operation guard still authenticated the request",
        transport: "MCP",
        bind_unsupported_because: "an operation takes one `Parameters<T>` struct, not the named \
                                   id argument the binding reads. Keep \
                                   `#[authorize(Action, Entity)]` and bind in the body with the \
                                   service's `access`",
    }
}

/// Every `Parameters<Valid<T>>` / `Parameters<Piped<P, T>>` argument, matched on
/// the normalized signature so each carries the ident the wrapper declares.
fn piped_args(sig: &Signature) -> syn::Result<Vec<PipedArg>> {
    let mut args = Vec::new();
    for input in &sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            continue;
        };
        let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
            continue;
        };
        // A bare `Valid<T>` outside `Parameters<…>` never reaches a body: rmcp
        // deserializes an operation's arguments through `Parameters`, so such a
        // parameter would be treated as a context extractor and fail to resolve.
        // Catching it here turns a bewildering rmcp trait error into one line.
        if pipe_wrapper(&pat_type.ty).is_some() {
            return Err(syn::Error::new_spanned(
                &pat_type.ty,
                "a pipe on an MCP operation wraps the *arguments*, so it goes inside \
                 `Parameters<…>` — write `Parameters<Valid<T>>` (or \
                 `Parameters<Piped<P, T>>`), which is what exposes `T` as the \
                 operation's schema",
            ));
        }
        let Some((ident, params)) = generic_args(&pat_type.ty) else {
            continue;
        };
        if ident != "Parameters" {
            continue;
        }
        let Some(carrier) = params.first().map(|ty| (*ty).clone()) else {
            continue;
        };
        let Some(wrapper) = pipe_wrapper(&carrier) else {
            continue;
        };
        let (pipe, value_ty) = match wrapper {
            PipeWrapper::Piped { pipe, value } => (Some(pipe), value),
            PipeWrapper::Valid { value } => (None, value),
        };
        args.push(PipedArg {
            ident: pat_ident.ident.clone(),
            pipe,
            value_ty,
        });
    }
    Ok(args)
}

/// The one thing an authored `#[mcp] impl` may not hold, spanned on the item
/// itself rather than on the type — a reader needs to see *which* one.
fn unsupported(entry: &ImplItem) -> syn::Error {
    syn::Error::new_spanned(
        entry,
        "#[tools] serves only #[tool] and #[prompt] methods — move anything else to \
         a plain `impl` block beside it",
    )
}
