//! `#[tools]` on an `impl` block — the operations half of the MCP decorator.
//!
//! The authored methods are re-emitted untouched; each operation gets a
//! generated wrapper carrying `#[tool(name = "…")]` that runs guards → gate →
//! pipes → call → mask, and rmcp's routers and `ServerHandler` are emitted in a
//! private child module that carries the `rmcp` import rmcp's macros resolve
//! against. An inherent impl may sit in any module of the defining crate and
//! still reach the parent's private fields (pinned by
//! `tests/integration/mcp_impl.rs`).

use nest_rs_codegen::pair;
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
    /// wrapper delegates to.
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
    reject_http_only_layers(&item.attrs, "MCP", "host")?;
    pair::MCP.reject_host_layers(&item.attrs)?;

    let self_ty = item.self_ty.clone();
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

    // `pub(crate)` carries the router out of the generated module: the
    // duplicate-tool boot check reads it from the parent.
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

    // Read back by the struct half's `Discoverable::injected`, so an unprovided
    // guard fails the boot rather than the first call.
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
    // `Guard::check_mcp` defaults to `Ok(())`: the bound makes a guard declare MCP.
    let capability_bounds =
        guard_capability_bounds(operation_guards(), quote!(::nest_rs_guards::McpGuard));
    let layers = layer_deps(operation_guards());
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

    // Each pipe carrier is replaced by the value it wraps, so rmcp derives the
    // tool's JSON Schema from `T`.
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
    let call_args = forwarded_arg_idents(sig)?;
    let call = await_if_async(sig, quote!(<#self_ty>::#name(self, #(#call_args),*)));
    let body = mask(posture, sig, call)?;
    wire_sig.asyncness = Some(syn::token::Async(name.span()));

    // A refusal becomes the operation's answer at one site, typed by
    // `OperationAnswer`, so a non-`Result` return is refused once, at the return type.
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
/// Always emitted, even when the operation declares none: the impl half cannot
/// see whether the host struct declared guards.
fn guard_chain(self_ty: &Type, op: &Operation, label: &LitStr) -> TokenStream2 {
    let method_specs = scoped_specs(&op.guards, quote!(dyn ::nest_rs_guards::Guard));
    let force = force_guard_typeids(&op.force_guards);
    let kind = op.role.kind();
    let name = LitStr::new(&op.name().to_string(), op.name().span());
    quote! {
        {
            static __NESTRS_GUARD_CHAIN: ::nest_rs_guards::__private::SiteChainCell =
                ::nest_rs_guards::__private::SiteChainCell::new();
            ::nest_rs_guards::run_layered_mcp_chain(
                &__NESTRS_GUARD_CHAIN,
                #label,
                ::core::any::type_name::<#self_ty>(),
                #kind,
                #name,
                &|| ::nest_rs_guards::__private::SiteChainSources {
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
            #apply.map_err(|__nestrs_err| ::nest_rs_mcp::pipe_error(&__nestrs_err))?,
        );
    }
}

/// Step 4 — the call, with response masking when the posture arms it.
///
/// `Json<T>` is unwrapped before masking and rewrapped after: rmcp's wrapper is
/// neither `Serialize` nor `DeserializeOwned`.
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

    // `take_operation` already refused a masked return not spelled `Result<…>`.
    let Some(ok_ty) = result_ok_type(sig) else {
        return Ok(call);
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
            // No `Ok(..?)` rewrap: clippy would flag it at every use site.
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
        let Some(index) =
            nest_rs_codegen::one_role_per_method("role", &method.attrs, &Role::names(), "")?
        else {
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
    // Left on the authored method, rmcp would route it as a second tool of the same name.
    method.attrs.remove(index);

    reject_http_only_layers(&method.attrs, "MCP", "operation")?;
    let guards = take_path_list(&mut method.attrs, "use_guards")?;
    let force_guards = take_path_list(&mut method.attrs, "force_guards")?;
    let posture = posture_rules(role).take(method)?;
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

    let mut sig = method.sig.clone();
    normalize_forwarded_args(sig.inputs.iter_mut())?;
    let pipes = piped_args(&sig)?;

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
                    !::nest_rs_mcp::description_is_blank(#expr),
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

/// MCP's half of the shared posture grammar, per operation role.
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
