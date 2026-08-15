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
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Path, Signature, Token, Type,
};

use nest_rs_codegen::{
    PipeWrapper, Posture, PostureRules, expr_str, force_guard_typeids, forwarded_arg_idents,
    generic_args, guard_capability_bounds, impl_self_ident, layer_deps, normalize_forwarded_args,
    nth_generic_type, pipe_wrapper, reject_http_only_layers, scoped_specs, snake_case,
    take_path_list, type_label,
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
#[derive(Clone, Copy)]
enum Role {
    Tool,
    Prompt,
}

impl Role {
    /// The role an attribute gives a method, if it gives one.
    fn from_attr(attr: &Attribute) -> Option<Self> {
        match () {
            () if attr.path().is_ident("tool") => Some(Self::Tool),
            () if attr.path().is_ident("prompt") => Some(Self::Prompt),
            _ => None,
        }
    }

    /// The attribute a reader wrote, for a diagnostic that quotes it back.
    fn attr(self) -> &'static str {
        match self {
            Self::Tool => "#[tool]",
            Self::Prompt => "#[prompt]",
        }
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
    /// The wrapper's own ident — never on the wire.
    wrapper: syn::Ident,
    /// The authored signature, with every argument pattern normalized to the
    /// plain identifier it binds.
    sig: Signature,
    guards: Vec<Path>,
    force_guards: Vec<Path>,
    posture: Posture,
    pipes: Vec<PipedArg>,
    /// Attributes the wrapper keeps beside its role attribute (`#[cfg]`,
    /// `#[allow]`, …) — the doc comment stays on the authored method, where a
    /// reader of the source looks for it.
    passthrough: Vec<Attribute>,
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
    if let Some(attr) = item.attrs.iter().find(|a| a.path().is_ident("use_guards")) {
        return Err(syn::Error::new_spanned(
            attr,
            "put `#[use_guards(...)]` on the host's `struct`, not its `impl` block — \
             uniform with `#[controller]`, `#[resolver]` and `#[gateway]`",
        ));
    }

    let self_ty = item.self_ty.clone();
    let base = impl_self_ident(&self_ty, "#[mcp]")?;
    let operations = take_operations(&mut item, &base)?;
    if operations.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[mcp] on an impl with no #[tool] or #[prompt] method has nothing to \
             mount — drop the decorator, or mark the methods it should serve",
        ));
    }

    let module = format_ident!("__nest_rs_mcp_{}", snake_case(&base.to_string()),);
    let generics = item.generics.split_for_impl();

    let wrappers = |ops: &[Operation]| -> syn::Result<Vec<TokenStream2>> {
        ops.iter().map(|op| wrapper(&self_ty, op)).collect()
    };
    let tools = wrappers(&operations.tools)?;
    let prompts = wrappers(&operations.prompts)?;

    // `vis = "pub(crate)"` is what carries rmcp's generated router out of this
    // module, so the boot checks can still read the host's tool names.
    let tool_impl = router_impl(
        &self_ty,
        &generics,
        &tools,
        quote!(tool_router(vis = "pub(crate)")),
    );
    let prompt_impl = router_impl(&self_ty, &generics, &prompts, quote!(prompt_router));

    let handler_attrs = {
        let tools = (!operations.tools.is_empty()).then(|| quote!(#[tool_handler]));
        let prompts = (!operations.prompts.is_empty()).then(|| quote!(#[prompt_handler]));
        quote!(#tools #prompts)
    };

    // Capabilities are *derived*, never restated: a router with methods in it is
    // the proof the surface exists, so a host cannot route tools it forgot to
    // advertise. A hand-written surface (resources, completion) is declared by
    // hand in its own `impl ServerHandler`, which is the one shape this does not
    // generate.
    let capabilities = {
        let tools = (!operations.tools.is_empty()).then(|| quote!(.enable_tools()));
        let prompts = (!operations.prompts.is_empty()).then(|| quote!(.enable_prompts()));
        quote!(ServerCapabilities::builder() #tools #prompts .build())
    };

    // Every guard any operation declared, deduped, for `Discoverable::injected`
    // — the struct half reads this back through `DefaultOperationLayers`, so a
    // guard no reachable module provides fails the boot naming it rather than
    // resolving to nothing at the first call.
    let operation_guards: Vec<&Path> = operations
        .all()
        .flat_map(|op| op.guards.iter().chain(op.force_guards.iter()))
        .collect();
    // Operation-scope guards run `Guard::check_mcp`, whose default is `Ok(())`.
    let capability_bounds = guard_capability_bounds(
        operation_guards.iter().copied(),
        quote!(::nest_rs_guards::McpGuard),
    );
    let layers = layer_deps(operation_guards.iter().copied());
    let layer_keys = &layers.keys;
    let layer_labels = &layers.labels;

    let (impl_generics, ty_generics, where_clause) = &generics;
    Ok(quote! {
        #item

        #capability_bounds

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
            use ::nest_rs_mcp::model::{ServerCapabilities, ServerInfo};
            use ::nest_rs_mcp::{
                ServerHandler, prompt, prompt_handler, prompt_router, tool, tool_handler,
                tool_router,
            };

            #tool_impl
            #prompt_impl

            #handler_attrs
            impl #impl_generics ServerHandler for #self_ty #ty_generics #where_clause {
                fn get_info(&self) -> ServerInfo {
                    ServerInfo::new(#capabilities)
                }
            }
        }
    })
}

/// One rmcp router impl carrying `methods`, or nothing when the host serves none
/// of that role.
fn router_impl(
    self_ty: &Type,
    (impl_generics, ty_generics, where_clause): &Generics<'_>,
    methods: &[TokenStream2],
    router: TokenStream2,
) -> Option<TokenStream2> {
    if methods.is_empty() {
        return None;
    }
    Some(quote! {
        #[#router]
        impl #impl_generics #self_ty #ty_generics #where_clause {
            #(#methods)*
        }
    })
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
        passthrough,
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
    let call = if sig.asyncness.is_some() {
        quote!(<#self_ty>::#name(self, #(#call_args),*).await)
    } else {
        quote!(<#self_ty>::#name(self, #(#call_args),*))
    };
    let body = mask(posture, sig, call)?;

    Ok(quote! {
        #role_attr
        #(#passthrough)*
        #wire_sig {
            #chain
            #gate
            #(#pipe_prelude)*
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

    // `take_operation` already refused a non-`Result` operation, so this is the
    // shape by construction rather than a second check with its own message.
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

/// Partition the decorated methods by role, taking each one's layer declarations
/// off the authored method as it goes.
///
/// The attributes are **consumed**: what stays on the re-emitted method is the
/// developer's own (`#[doc]`, `#[allow]`, …), and nothing the compiler would
/// reject as unknown.
fn take_operations(item: &mut ItemImpl, base: &syn::Ident) -> syn::Result<Operations> {
    let mut operations = Operations::default();

    for entry in item.items.iter_mut() {
        let ImplItem::Fn(method) = entry else {
            return Err(unsupported(entry));
        };
        let Some((index, role)) = method
            .attrs
            .iter()
            .enumerate()
            .find_map(|(index, attr)| Role::from_attr(attr).map(|role| (index, role)))
        else {
            // Helpers belong beside the struct: left here they would move into
            // the generated module, where a reader would not look for them.
            return Err(unsupported(entry));
        };

        let operation = take_operation(method, index, role, base)?;
        match role {
            Role::Tool => operations.tools.push(operation),
            Role::Prompt => operations.prompts.push(operation),
        }
    }

    Ok(operations)
}

/// Everything one decorated method declares, taken off it.
fn take_operation(
    method: &mut ImplItemFn,
    index: usize,
    role: Role,
    base: &syn::Ident,
) -> syn::Result<Operation> {
    let name = method.sig.ident.clone();
    let role_attr = wrapper_role_attr(method, index, role)?;
    // Removed after `wrapper_role_attr` has read the doc comment off it: the
    // attribute belongs to the wrapper now, and leaving a copy behind would make
    // rmcp route the authored method as a second tool of the same name.
    method.attrs.remove(index);

    reject_http_only_layers(&method.attrs, "MCP", "operation")?;
    // The guard chain and the access gate both refuse by returning, so there has
    // to be a channel for a refusal. Named here rather than left to surface as a
    // `?`-on-a-non-Result deep inside the expansion.
    if result_ok_type(&method.sig).is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            format!(
                "an MCP operation returns `Result<_, McpError>` — its guard chain \
                 and its access posture both refuse by returning, and a {} that \
                 cannot fail has nowhere to report a denial",
                role.attr(),
            ),
        ));
    }
    let guards = take_path_list(&mut method.attrs, "use_guards", "guard")?;
    let force_guards = take_path_list(&mut method.attrs, "force_guards", "guard")?;
    let posture = posture_rules(role).take(method)?;

    // The developer's method keeps its own patterns — this normalizes a clone, so
    // the wrapper has one plain ident per argument to declare and forward by.
    let mut sig = method.sig.clone();
    normalize_forwarded_args(sig.inputs.iter_mut())?;
    let pipes = piped_args(&sig)?;

    // `#[cfg]` and `#[allow]` have to reach the wrapper too — it is a separate
    // item, and a `#[cfg]`-ed-out operation whose wrapper survived would not
    // compile. A doc comment stays behind: it is the authored method's prose for
    // a reader, and the wrapper already carries the model's copy as
    // `description`.
    let passthrough: Vec<Attribute> = method
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("allow"))
        .cloned()
        .collect();

    Ok(Operation {
        role,
        role_attr,
        wrapper: format_ident!("__nestrs_mcp_{}_{}", snake_case(&base.to_string()), name),
        sig,
        guards,
        force_guards,
        posture,
        pipes,
        passthrough,
    })
}

/// The `#[tool]` / `#[prompt]` attribute as the wrapper will carry it: the
/// author's own arguments, plus the `name` the protocol must keep addressing
/// and — only when the attribute states none — a `description` lifted from the
/// doc comment. An operation with neither is a compile error: the description
/// is what a model reads to choose between operations, so it is not optional.
fn wrapper_role_attr(method: &ImplItemFn, index: usize, role: Role) -> syn::Result<TokenStream2> {
    let attr = &method.attrs[index];
    let stated = stated_keys(attr)?;
    let role_path = attr.path().clone();

    let mut extra: Vec<TokenStream2> = Vec::new();
    if !stated.iter().any(|key| key == "description") {
        // The attribute is the declared form — the prose is a value the
        // decorator compiles in, which is what lets a codebase that carries no
        // comments still describe its tools. The doc comment is the fallback
        // for one that does, so the sentence is never written twice.
        let Some(doc) = doc_comment(&method.attrs) else {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                "an MCP operation needs a description — the model reads it to choose \
                 between operations. Write a doc comment above it, or state \
                 `description = \"…\"` on the attribute",
            ));
        };
        extra.push(quote!(description = #doc));
    }
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
    Ok(quote!(#[#role_path(#rest #(#extra),*)]))
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

/// The method's doc comment, joined into the one sentence rmcp sends.
fn doc_comment(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(value) => expr_str(&value.value).ok(),
            _ => None,
        })
        .map(|literal| literal.value().trim().to_owned())
        .collect();

    let joined = lines.join(" ");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// MCP's half of the shared posture grammar, per operation role so the refusal
/// quotes back the attribute the developer actually wrote.
fn posture_rules(role: Role) -> PostureRules {
    PostureRules {
        operation: role.attr(),
        public_means: "no gate and no mask — `#[use_guards]` guards still run, and the \
                       endpoint's own operation guard still authenticated the request",
        bind_unsupported: "`bind = Service` is not available on MCP — an operation takes one \
                           `Parameters<T>` struct, not the named id argument the binding reads. \
                           Keep `#[authorize(Action, Entity)]` and bind in the body with the \
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
        "#[mcp] serves only #[tool] and #[prompt] methods — move anything else to \
         a plain `impl` block beside it",
    )
}
