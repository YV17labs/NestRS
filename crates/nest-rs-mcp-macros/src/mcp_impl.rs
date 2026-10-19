//! `#[tools]` on an `impl` block — the operations half of the MCP decorator.
//!
//! The authored methods are re-emitted untouched; each operation declares its
//! `static` site — its guards, its posture's class gate, its exception filters
//! and interceptors — and gets a generated wrapper carrying
//! `#[tool(name = "…")]` that dispatches it through that site, its tail running
//! pipes → `Scoped<T>` parameters → call → mask. rmcp's routers and
//! `ServerHandler` are emitted in a private child module that carries the
//! `rmcp` import rmcp's macros resolve against. An inherent impl may sit in any
//! module of the defining crate and still reach the parent's private fields
//! (pinned by `tests/integration/mcp_impl.rs`).

use nest_rs_codegen::__private::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, FnArg, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Path, Signature, Token, Type,
};

use nest_rs_codegen::__private::layers::{LayerScope, Layers, take_layers};
use nest_rs_codegen::__private::{
    Collision, Conditional, DispatchKeys, HandlerSite, HostBorrow, HostMeta, PipeWrapper, Posture,
    PostureRules, answer_span, await_if_async, cfg_attrs, concrete_signature, force_guard_typeids,
    forwarded_arg_idents, generic_args, guard_capability_bounds, handler_const, handler_ref,
    impl_self_ident, layer_deps, normalize_forwarded_args, nth_generic_type, one_role_per_method,
    pipe_wrapper, reject_meta_on_operations, scoped_specs, shared_receiver, site_const_ident,
    site_fn_ident, ungrouped_expr,
};
use nest_rs_codegen::snake_case;

/// The decorated methods of one authored `impl`, partitioned by router.
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
    /// Every role with its bare attribute name and the bracketed form a
    /// diagnostic quotes.
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

    /// What rmcp's router takes this role's answer as, named where the
    /// expansion cannot spell the answer's type.
    fn wire_answer(self) -> TokenStream2 {
        match self {
            Self::Tool => quote!(::nest_rs_mcp::__private::IntoCallToolResult),
            Self::Prompt => quote!(::nest_rs_mcp::__private::IntoGetPromptResult),
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
    interceptors: Vec<Path>,
    exception_filters: Vec<Path>,
    posture: Posture,
    pipes: Vec<PipedArg>,
    /// Its `Scoped<T>` parameters, resolved in its tail rather than by rmcp.
    scoped: Vec<ScopedArg>,
    /// The authored method's `#[cfg]` conditions, which its router group, and so
    /// its wrapper and its route, are compiled under.
    cfgs: Vec<TokenStream2>,
    /// The name a client calls it by.
    wire_name: String,
    /// The method's `#[meta]` values.
    metas: Vec<syn::Expr>,
}

impl Operation {
    /// The authored method's name: what the protocol addresses, and what the
    /// wrapper delegates to.
    fn name(&self) -> &syn::Ident {
        &self.sig.ident
    }
}

/// A `#[tool]` / `#[prompt]` parameter a request-scoped provider fills.
struct ScopedArg {
    ident: syn::Ident,
    /// `Scoped<T>`, as written.
    ty: Type,
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
    if let Err(err) = pair::MCP.reject_args(
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
    pair::MCP.reject_host_layers(&item.attrs)?;
    reject_meta_on_operations(&pair::MCP, &item.attrs)?;

    let self_ty = item.self_ty.clone();
    let base = impl_self_ident(&self_ty, "#[tools]")?;
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "`#[tools] impl` must be on a concrete self type — each operation declares its \
             site in a `static`, which no parameter of the impl reaches, so a generic host's \
             instances would share one site",
        ));
    }
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

    // `pub(crate)` carries each router out of the generated module: the mount
    // reads both from the parent, to route a name and check it is claimed once.
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
            vis: quote!(pub(crate)),
        },
    )?;

    let handler_attrs = {
        let tools = (!operations.tools.is_empty()).then(|| quote!(#[tool_handler]));
        let prompts = (!operations.prompts.is_empty()).then(|| quote!(#[prompt_handler]));
        quote!(#tools #prompts)
    };

    // Each operation switches its surface on under its own `#[cfg]`, so a host
    // whose every tool is compiled out advertises no tools capability.
    let capabilities = {
        let surface = |ops: &[Operation], field: TokenStream2| {
            ops.iter()
                .map(|op| {
                    let cfgs = &op.cfgs;
                    // A `let`: `#[cfg]` may sit on a `let` statement, not on an assignment.
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

    // `Guard::check` admits by default: the bound makes a guard declare MCP.
    let capability_bounds = guard_capability_bounds(
        operations.all().flat_map(|op| {
            op.guards.iter().map(|item| Conditional {
                cfgs: &op.cfgs,
                item,
            })
        }),
        quote!(::nest_rs_guards::GuardFor<::nest_rs_mcp::McpView>),
    );
    // Read back by the struct half's `Discoverable::injected`, so a layer no
    // imported module provides fails the boot rather than the first call.
    let layers = layer_deps(operations.all().flat_map(|op| {
        op.guards
            .iter()
            .chain(&op.interceptors)
            .chain(&op.exception_filters)
            .map(|item| Conditional {
                cfgs: &op.cfgs,
                item,
            })
    }));
    let description_checks = operations.all().filter_map(|op| {
        let cfgs = &op.cfgs;
        op.description_check
            .as_ref()
            .map(|check| quote!(#(#cfgs)* #check))
    });
    let layer_keys = &layers.keys;
    let layer_labels = &layers.labels;
    let sites = site_items(&self_ty, &base, &operations);

    let (impl_generics, ty_generics, where_clause) = &generics;
    Ok(quote! {
        #item

        #capability_bounds

        #(#description_checks)*

        #markers

        #sites

        impl #impl_generics #self_ty #ty_generics #where_clause {
            #[doc(hidden)]
            pub fn __nestrs_mcp_operation_layers()
                -> (::std::vec::Vec<::core::any::TypeId>, ::std::vec::Vec<&'static str>)
            {
                (::std::vec![#(#layer_keys),*], ::std::vec![#(#layer_labels),*])
            }

            // Read by the struct half: this host's routers are every tool and
            // prompt it serves, since `#[tools]` writes its whole handler.
            #[doc(hidden)]
            pub fn __nestrs_mcp_declares_every_route() -> bool {
                true
            }
        }

        #[doc(hidden)]
        mod #module {
            use super::*;

            // rmcp's expansions emit bare `rmcp::` paths resolved at the call site.
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
/// One rmcp router impl per set of `#[cfg]` conditions, merged into one router
/// function: rmcp's router attribute routes a method whatever its `#[cfg]`, so
/// only a condition on the impl removes a compiled-out route with its method.
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
        // rmcp's router attributes read `router = …` as a string.
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

/// The three halves of `Generics::split_for_impl`.
type Generics<'a> = (
    syn::ImplGenerics<'a>,
    syn::TypeGenerics<'a>,
    Option<&'a syn::WhereClause>,
);

/// The generated method that rmcp routes to: the operation dispatched through
/// its site, the authored method its tail.
fn wrapper(self_ty: &Type, op: &Operation) -> syn::Result<TokenStream2> {
    let Operation {
        role,
        role_attr,
        wrapper,
        sig,
        posture,
        pipes,
        scoped,
        wire_name,
        ..
    } = op;
    let name = op.name();

    // Each pipe carrier is replaced by the value it wraps, so rmcp derives the
    // tool's JSON Schema from `T`; a `Scoped<T>` is the tail's, never rmcp's.
    let mut wire_sig = sig.clone();
    wire_sig.ident = wrapper.clone();
    wire_sig.asyncness = Some(syn::token::Async(name.span()));
    wire_sig.inputs = sig
        .inputs
        .iter()
        .filter_map(|input| {
            let mut input = input.clone();
            if let FnArg::Typed(pat_type) = &mut input
                && let syn::Pat::Ident(pat_ident) = &*pat_type.pat
            {
                if scoped.iter().any(|arg| arg.ident == pat_ident.ident) {
                    return None;
                }
                if let Some(arg) = pipes.iter().find(|arg| arg.ident == pat_ident.ident) {
                    let value = &arg.value_ty;
                    *pat_type.ty = syn::parse_quote!(::nest_rs_mcp::Parameters<#value>);
                }
            }
            Some(input)
        })
        .collect();
    // Whatever error the method fails with, rmcp is answered a `Result` over an
    // `McpError`: spelled out where the method spells its `Result`, so rmcp
    // reads a `Json<T>` off it, and behind an opaque type where an alias or a
    // renamed `Result` hides the value's type from the expansion.
    wire_sig.output = match spelled_result(sig) {
        Some(ok) => syn::parse_quote!(
            -> ::core::result::Result<#ok, ::nest_rs_mcp::McpError>
        ),
        None => {
            let answer = role.wire_answer();
            syn::parse_quote_spanned!(answer_span(sig)=> -> impl #answer + use<>)
        }
    };

    let pipe_prelude = pipes.iter().map(pipe_statement);
    let scoped_prelude = scoped.iter().map(|ScopedArg { ident, ty }| {
        quote! { let #ident: #ty = ::nest_rs_mcp::__private::scoped()?; }
    });
    let call_args = forwarded_arg_idents(sig)?;
    let call = await_if_async(sig, quote!(<#self_ty>::#name(self, #(#call_args),*)));
    // What the method answers is read by its type, refused at its return type.
    let returned_by = quote_spanned!(answer_span(sig)=> __nestrs_answered);
    let answer = quote_spanned! {answer_span(sig)=>
        ::nest_rs_mcp::__private::answer_of(#returned_by)
    };
    let answered = masked(posture, sig)?;
    let site = site_ref(self_ty, name);
    let kind = role.kind();
    let wire_name = LitStr::new(wire_name, name.span());
    Ok(quote! {
        #role_attr
        #wire_sig {
            ::nest_rs_mcp::__private::run_operation(
                #site,
                ::nest_rs_mcp::__private::operation_view(
                    #kind,
                    #wire_name,
                    ::core::any::type_name::<#self_ty>(),
                ),
                move || async move {
                    #(#pipe_prelude)*
                    #(#scoped_prelude)*
                    let #returned_by = #call;
                    match #answer {
                        ::core::result::Result::Ok(__nestrs_value) => #answered,
                        ::core::result::Result::Err(__nestrs_error) => {
                            #[allow(unused_imports)]
                            use ::nest_rs_mcp::__private::ErrorReportChain as _;
                            ::core::result::Result::Err(
                                ::nest_rs_mcp::__private::ErrorReport(__nestrs_error)
                                    .into_failure(),
                            )
                        }
                    }
                },
            )
            .await
        }
    })
}

/// The `T` of a return spelled `Result<T, E>`, two arguments written; `None`
/// for an alias or a renamed `Result`, whose value's type only its type says.
fn spelled_result(sig: &Signature) -> Option<&Type> {
    let syn::ReturnType::Type(_, ty) = &sig.output else {
        return None;
    };
    let (ident, args) = generic_args(ty)?;
    (ident == "Result" && args.len() == 2).then(|| args[0])
}

/// The `&'static Site` `method`'s operation runs through on `self_ty`.
fn site_ref(self_ty: &Type, method: &syn::Ident) -> TokenStream2 {
    let name = site_const_ident("site", method);
    quote! { <#self_ty>::#name }
}

/// Step 2 — the class gate `#[authorize(Action, Entity)]` desugars to, every
/// edge's one, run on the operation its guards checked.
fn class_gate(posture: &Posture) -> TokenStream2 {
    match posture {
        Posture::Authorize { action, entity, .. } => quote! {
            ::core::option::Option::Some(
                ::nest_rs_authz::class_gate::<#action, #entity>
                    as ::nest_rs_core::__private::ClassGate,
            )
        },
        Posture::Public => quote!(::core::option::Option::None),
    }
}

/// Step 3 — run one argument's pipe over its wire value and rebind the parameter
/// to the carrier the body expects.
///
/// Runs after the gate, so a validation message is never an existence oracle.
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
            #apply.map_err(::nest_rs_mcp::__private::OperationFailure::from)?,
        );
    }
}

/// Step 4 — the answer's value, `__nestrs_value`, with response masking when
/// the posture arms it.
///
/// `Json<T>` is unwrapped before masking and rewrapped after: rmcp's wrapper is
/// neither `Serialize` nor `DeserializeOwned`.
fn masked(posture: &Posture, sig: &Signature) -> syn::Result<TokenStream2> {
    let unmasked = quote!(::core::result::Result::Ok(__nestrs_value));
    let Posture::Authorize {
        action,
        entity,
        unmasked: declared_unmasked,
    } = posture
    else {
        return Ok(unmasked);
    };
    if *declared_unmasked {
        return Ok(unmasked);
    }

    // `take_operation` already refused a masked return not spelled `Result<…>`.
    let Some(ok_ty) = result_ok_type(sig) else {
        return Ok(unmasked);
    };
    // Opaque content blocks: refused here, or an unmet `DeserializeOwned` bound
    // surfaces deep inside the expansion.
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

    if let Some(inner) = nth_generic_type(ok_ty, "Json", 0) {
        return Ok(quote! {{
            let ::nest_rs_mcp::Json(__nestrs_out) = __nestrs_value;
            ::nest_rs_authz::mcp::masked_value_for::<#action, #entity, #inner>(__nestrs_out)
                .map(::nest_rs_mcp::Json)
                .map_err(::nest_rs_mcp::__private::OperationFailure::from)
        }});
    }

    Ok(quote! {
        ::nest_rs_authz::mcp::masked_value_for::<#action, #entity, _>(__nestrs_value)
            .map_err(::nest_rs_mcp::__private::OperationFailure::from)
    })
}

/// The `T` of a `-> Result<T, E>` return, or `None` for anything else.
fn result_ok_type(sig: &Signature) -> Option<&Type> {
    let syn::ReturnType::Type(_, ty) = &sig.output else {
        return None;
    };
    nth_generic_type(ty, "Result", 0)
}

/// Partition the decorated methods by role, consuming each one's layer
/// attributes so the re-emitted method carries none rustc would reject.
fn take_operations(
    item: &mut ItemImpl,
    base: &syn::Ident,
) -> syn::Result<(Operations, DispatchKeys)> {
    let mut operations = Operations::default();
    // rmcp's router keeps the last route added under a name, silently.
    let mut declared = DispatchKeys::new(
        "#[tools]",
        "a host routes each tool name, and each prompt name, to one method, so one of the \
         two would never run — give one a distinct `name`",
    );

    for entry in item.items.iter_mut() {
        let ImplItem::Fn(method) = entry else {
            return Err(unsupported(entry));
        };
        let Some(index) = one_role_per_method("role", &method.attrs, &Role::names(), "")? else {
            return Err(unsupported(entry));
        };
        let Some(role) = Role::from_attr(&method.attrs[index]) else {
            return Err(unsupported(entry));
        };
        shared_receiver(method, "#[tools]", base, HostBorrow::Host)?;
        concrete_signature(method, "#[tools]")?;

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

        let operation = take_operation(method, index, role, base, wire_name)?;
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
    wire_name: String,
) -> syn::Result<Operation> {
    let name = method.sig.ident.clone();
    let (role_attr, description_check) = wrapper_role_attr(method, index, role)?;
    // Left on the authored method, rmcp would route it as a second tool of the same name.
    method.attrs.remove(index);

    let Layers {
        guards,
        force_guards,
        interceptors,
        exception_filters,
        meta: metas,
        ..
    } = take_layers(&pair::MCP, LayerScope::Method, &mut method.attrs)?;
    let posture = posture_rules(role).take(method)?;
    if posture.masks() && result_ok_type(&method.sig).is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            format!(
                "a masked {} spells its return `Result<…>`: the mask reads the \
                 value's shape — `Json<T>` to unwrap, `CallToolResult` to refuse — off \
                 that spelling, and a `Result` renamed on import or behind an alias hides \
                 it. Write `Result<…>`, or declare `#[authorize(Action, Entity, unmasked)]` \
                 and mask in the body",
                role.attr(),
            ),
        ));
    }

    let mut sig = method.sig.clone();
    normalize_forwarded_args(sig.inputs.iter_mut())?;
    let pipes = piped_args(&sig)?;
    let scoped = scoped_args(&sig);

    let cfgs = cfg_attrs(&method.attrs);

    Ok(Operation {
        role,
        role_attr,
        description_check,
        wrapper: format_ident!("__nestrs_mcp_{}_{}", snake_case(&base.to_string()), name),
        sig,
        guards,
        force_guards,
        interceptors,
        exception_filters,
        posture,
        pipes,
        scoped,
        cfgs,
        wire_name,
        metas,
    })
}

/// Every `Scoped<T>` parameter, read by its last segment as `#[routes]` reads
/// HTTP's: the tail resolves it once the guards and the gate admitted the
/// caller, so the wire signature rmcp extracts from leaves it out.
fn scoped_args(sig: &Signature) -> Vec<ScopedArg> {
    sig.inputs
        .iter()
        .filter_map(|input| {
            let FnArg::Typed(pat_type) = input else {
                return None;
            };
            let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
                return None;
            };
            generic_args(&pat_type.ty)
                .is_some_and(|(ident, _)| ident == "Scoped")
                .then(|| ScopedArg {
                    ident: pat_ident.ident.clone(),
                    ty: (*pat_type.ty).clone(),
                })
        })
        .collect()
}

/// What each operation declares on its host: its `Handler`, `mcp.operation`
/// addressed by the name a client calls, and its `static` site — the layers
/// its host and its method bind, its guard stage and its posture's class
/// gate — plus the list of sites the boot composes and the providers its
/// `Scoped<T>` parameters resolve, which the access graph checks.
fn site_items(self_ty: &Type, base: &syn::Ident, operations: &Operations) -> TokenStream2 {
    let items = operations.all().map(|op| {
        let wire_name = &op.wire_name;
        let method = op.name();
        let cfgs = &op.cfgs;
        let handler = handler_const(&HandlerSite {
            unit: quote!(::nest_rs_mcp::unit::OPERATION),
            self_ty,
            host_name: base,
            method,
            address: quote!(#wire_name),
            posture: op.posture.site_posture(),
            host_meta: HostMeta::Struct,
            method_meta: &op.metas,
            cfgs,
        });
        let handler_ref = handler_ref(self_ty, method);
        let layers_fn = site_fn_ident("layers", method);
        let guards_fn = site_fn_ident("guards", method);
        let site = site_const_ident("site", method);
        let interceptors = &op.interceptors;
        let exception_filters = &op.exception_filters;
        let method_guards = scoped_specs(&op.guards, quote!(dyn ::nest_rs_guards::Guard));
        let force = force_guard_typeids(&op.force_guards);
        let kind = op.role.kind();
        let gate = class_gate(&op.posture);
        quote! {
            #handler

            #(#cfgs)*
            #[doc(hidden)]
            fn #layers_fn() -> ::nest_rs_core::__private::SiteLayers {
                ::nest_rs_core::__private::SiteLayers {
                    host_interceptors: <#self_ty>::__nestrs_mcp_host_interceptor_specs(),
                    method_interceptors: ::std::vec![
                        #(::nest_rs_core::interceptor::<#interceptors>()),*
                    ],
                    host_exception_filters:
                        <#self_ty>::__nestrs_mcp_host_exception_filter_specs(),
                    method_exception_filters: ::std::vec![
                        #(::nest_rs_core::exception_filter::<#exception_filters>()),*
                    ],
                }
            }

            #(#cfgs)*
            #[doc(hidden)]
            fn #guards_fn(
                __nestrs_container: &::nest_rs_core::Container,
                __nestrs_handler: &::nest_rs_core::Handler,
            ) -> ::nest_rs_core::anyhow::Result<
                ::core::option::Option<
                    ::std::sync::Arc<dyn ::nest_rs_core::__private::GuardStage>,
                >,
            > {
                ::nest_rs_mcp::__private::operation_guard_stage(
                    __nestrs_container,
                    __nestrs_handler,
                    ::nest_rs_mcp::__private::OperationGuards {
                        host: <#self_ty>::__nestrs_mcp_host_guard_specs(),
                        method: #method_guards,
                        force: #force,
                        kind: #kind,
                        host_type: ::core::any::type_name::<#self_ty>(),
                    },
                )
            }

            #(#cfgs)*
            #[doc(hidden)]
            pub const #site: &'static ::nest_rs_core::__private::Site = {
                static __NESTRS_SITE: ::nest_rs_core::__private::Site =
                    ::nest_rs_core::__private::declare_site(
                        #handler_ref,
                        ::nest_rs_core::__private::SiteDeclaration {
                            layers: <#self_ty>::#layers_fn,
                            global: ::nest_rs_mcp::__private::OPERATION_LAYERS,
                            guard_stage: ::core::option::Option::Some(
                                <#self_ty>::#guards_fn
                                    as ::nest_rs_core::__private::GuardStageFactory,
                            ),
                            class_gate: #gate,
                        },
                    );
                &__NESTRS_SITE
            };
        }
    });
    let site_pushes = operations.all().map(|op| {
        let cfgs = &op.cfgs;
        let site = site_ref(self_ty, op.name());
        quote! {
            #(#cfgs)*
            __nestrs_sites.push(#site);
        }
    });
    let probes = operations.all().flat_map(|op| {
        op.scoped.iter().map(move |ScopedArg { ty, .. }| {
            let cfgs = &op.cfgs;
            quote! {
                #(#cfgs)*
                let () = __nestrs_resolved.extend(
                    (&&::nest_rs_core::__private::ParamProbe::<#ty>::new()).resolved_provider(),
                );
            }
        })
    });
    quote! {
        impl #self_ty {
            #(#items)*

            /// The site each operation runs, for the boot to compose.
            #[doc(hidden)]
            pub fn __nestrs_mcp_operation_sites()
                -> ::std::vec::Vec<&'static ::nest_rs_core::__private::Site>
            {
                #[allow(unused_mut, reason = "every operation may be compiled out")]
                let mut __nestrs_sites = ::std::vec::Vec::new();
                #(#site_pushes)*
                __nestrs_sites
            }

            /// The provider each `Scoped<T>` parameter resolves, asked of the
            /// parameter's type, for the access graph.
            #[doc(hidden)]
            pub fn __nestrs_mcp_injected_per_request()
                -> ::std::vec::Vec<(::core::any::TypeId, &'static str)>
            {
                #[allow(
                    unused_imports,
                    reason = "the probe's two arms are in scope for autoref, which no lint sees"
                )]
                use ::nest_rs_core::__private::{ParamProbeFallback as _, ParamProbeResolves as _};
                #[allow(unused_mut, reason = "an operation may take no `Scoped<T>`")]
                let mut __nestrs_resolved = ::std::vec::Vec::new();
                #(#probes)*
                __nestrs_resolved
            }
        }
    }
}

/// The sentence an operation with no description is refused with — at expansion
/// when the prose is a literal, and by the compiler when only a constant
/// evaluation can read it.
const NEEDS_A_DESCRIPTION: &str = "an MCP operation needs a description — the model reads it to \
                                   choose between operations. Write a doc comment above it, or \
                                   state `description = \"…\"` on the attribute";

/// The `#[tool]` / `#[prompt]` attribute as the wrapper will carry it: the
/// author's arguments, the authored `name`, and — when none is stated — a
/// `description` lifted from the doc comment.
///
/// A blank description is refused here when it is a literal; the second element
/// is a `const` refusing a macro or constant that evaluates blank; a runtime
/// expression is rmcp's to send as written.
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
                    !::nest_rs_mcp::__private::description_is_blank(#expr),
                    #NEEDS_A_DESCRIPTION,
                );
            })
        }
    };
    if !stated.iter().any(|key| key == "name") {
        // The wrapper's ident is generated; the wire keeps the authored name.
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
    Ok(Some(match ungrouped_expr(value) {
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

/// The top-level keys the author already stated inside `#[tool(...)]`; a nested
/// group like `annotations(…)` stays opaque, so rmcp's grammar is not restated.
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

/// The method's doc comment, joined into the one sentence rmcp sends: a literal
/// when every line is one, else a `concat!` keeping `#[doc = include_str!(…)]` lines.
fn doc_comment(attrs: &[Attribute]) -> Option<TokenStream2> {
    let lines: Vec<&syn::Expr> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(value) => Some(&value.value),
            _ => None,
        })
        .collect();
    let literal = |line: &syn::Expr| match ungrouped_expr(line) {
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

/// MCP's half of the shared posture grammar, per operation role.
fn posture_rules(role: Role) -> PostureRules {
    PostureRules {
        operation: role.attr(),
        public_means: "no gate and no mask — `#[use_guards]` guards still run, and the \
                       guards at the endpoint's request already authenticated it",
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
        // A bare pipe carrier is taken by rmcp for a context extractor, which fails
        // with an unreadable trait error.
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

/// An item `#[tools]` refuses, spanned on the item itself.
fn unsupported(entry: &ImplItem) -> syn::Error {
    syn::Error::new_spanned(
        entry,
        "#[tools] serves only #[tool] and #[prompt] methods — move anything else to \
         a plain `impl` block beside it",
    )
}
