//! `#[tools]` on an `impl` block — the operations half of the MCP decorator.
//!
//! The authored methods are re-emitted untouched; each operation declares its
//! `static` site — its guards, its posture's class gate, its exception filters
//! and interceptors — and gets a generated wrapper that dispatches it through
//! that site, its tail running pipes → `Scoped<T>` and `Ambient<T>` parameters →
//! call → mask.
//! A tool's or a prompt's wrapper carries `#[tool(name = "…")]` /
//! `#[prompt(name = "…")]` for rmcp's routers; a resource's is called by the
//! `read_resource` and `list_resources` the expansion writes itself, rmcp
//! having no router for them (`resource.rs`). rmcp's routers and
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
    Collision, Conditional, DispatchKeys, HandlerSite, HostBorrow, HostMeta, Posture, PostureRules,
    TailParam, answer_span, await_if_async, cfg_attrs, concrete_signature, force_guard_typeids,
    forwarded_arg_idents, generic_args, guard_capability_bounds, handler_const, handler_ref,
    impl_self_ident, layer_deps, normalize_forwarded_args, nth_generic_type, one_role_per_method,
    piped_input, reject_meta_on_operations, scoped_specs, shared_receiver, site_const_ident,
    site_fn_ident, tail_param, ungrouped_expr, validate_argument,
};
use nest_rs_codegen::snake_case;

use crate::resource::{self, ResourceDecl};

/// The decorated methods of one authored `impl`, partitioned by router.
#[derive(Default)]
struct Operations {
    tools: Vec<Operation>,
    prompts: Vec<Operation>,
    /// The `#[resource]` reads, in declaration order: the order a URI is
    /// offered to their templates.
    resources: Vec<Operation>,
    /// The `#[resources]` listing — one, unless `#[cfg]` keeps one of several.
    listings: Vec<Operation>,
}

impl Operations {
    fn is_empty(&self) -> bool {
        self.tools.is_empty()
            && self.prompts.is_empty()
            && self.resources.is_empty()
            && self.listings.is_empty()
    }

    fn all(&self) -> impl Iterator<Item = &Operation> {
        self.tools
            .iter()
            .chain(self.prompts.iter())
            .chain(self.resources.iter())
            .chain(self.listings.iter())
    }
}

/// Which router a decorated method belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Tool,
    Prompt,
    Resource,
    Resources,
}

impl Role {
    /// Every role with its bare attribute name and the bracketed form a
    /// diagnostic quotes.
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Tool, "tool", "#[tool]"),
        (Self::Prompt, "prompt", "#[prompt]"),
        (Self::Resource, "resource", "#[resource]"),
        (Self::Resources, "resources", "#[resources]"),
    ];

    /// The bare attribute names, for the refusal that lists what is accepted
    /// and the markers a refused block still strips.
    pub(crate) fn names() -> [&'static str; 4] {
        Self::ALL.map(|(_, name, _)| name)
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
            Self::Resource => quote!(::nest_rs_mcp::McpOperationKind::Resource),
            Self::Resources => quote!(::nest_rs_mcp::McpOperationKind::Resources),
        }
    }

    /// What the wrapper turns this role's answer into, named where the
    /// expansion cannot spell the answer's type: rmcp's router's trait for a
    /// tool and a prompt, the framework's for a resource read and listing.
    fn wire_answer(self) -> TokenStream2 {
        match self {
            Self::Tool => quote!(::nest_rs_mcp::__private::IntoCallToolResult),
            Self::Prompt => quote!(::nest_rs_mcp::__private::IntoGetPromptResult),
            Self::Resource => quote!(::nest_rs_mcp::__private::ResourceBody),
            Self::Resources => quote!(::nest_rs_mcp::__private::ResourceListing),
        }
    }

    /// The word a chain label and a log field spell this role with.
    fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Prompt => "prompt",
            Self::Resource => "resource",
            Self::Resources => "resources",
        }
    }

    /// Whether rmcp routes this role, through the wrapper's own attribute.
    fn routed_by_rmcp(self) -> bool {
        matches!(self, Self::Tool | Self::Prompt)
    }

    /// Why `#[authorize(...)]` cannot mask this role's answer, whatever its
    /// type; `None` for a role whose answer may be a row.
    fn unmaskable(self) -> Option<&'static str> {
        match self {
            Self::Tool | Self::Resource => None,
            Self::Prompt => Some(
                "#[prompt] — its messages are prose, opaque to the entity round-trip. Keep the \
                 gate and write into them only what the caller may read",
            ),
            Self::Resources => Some(
                "#[resources] listing — its entries are URIs and names, opaque to the entity \
                 round-trip. Keep the gate and list only what the caller may read",
            ),
        }
    }

    /// What the mask unwraps off a maskable role's return, as a diagnostic
    /// offers it.
    fn mask_unwraps(self) -> &'static str {
        match self {
            Self::Resource => "`Json<T>` or `Option<Json<T>>`",
            Self::Tool | Self::Prompt | Self::Resources => "`Json<T>`",
        }
    }

    /// The content types this role answers that the entity round-trip cannot
    /// read, by their last segment: a masked return naming one is refused.
    /// The first is the one a diagnostic names.
    fn opaque_answers(self) -> &'static [&'static str] {
        match self {
            Self::Tool => &[
                "CallToolResult",
                "CallToolResponse",
                "ContentBlock",
                "InputRequiredResult",
            ],
            Self::Resource => &["ReadResourceResult"],
            Self::Prompt | Self::Resources => &[],
        }
    }
}

/// One decorated operation: what the developer wrote, and everything the wrapper
/// has to know to run the four steps around it.
struct Operation {
    role: Role,
    /// The role attribute as it will sit on the wrapper — description filled in
    /// from the doc comment, `name` pinned to the authored method's own; empty
    /// for a resource role, which rmcp does not route.
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
    /// Its `Scoped<T>` and `Ambient<T>` parameters, resolved in its tail rather
    /// than by rmcp.
    tail: Vec<TailArg>,
    /// The authored method's `#[cfg]` conditions, which its router group, and so
    /// its wrapper and its route, are compiled under.
    cfgs: Vec<TokenStream2>,
    /// The name a client calls it by — for a `#[resource]`, the template it
    /// is read through; for the `#[resources]` listing, its method's name.
    wire_name: String,
    /// The method's `#[meta]` values.
    metas: Vec<syn::Expr>,
    /// A `#[resource]`'s template and the parameters its variables fill.
    resource: Option<ResourceDecl>,
}

impl Operation {
    /// The authored method's name: what the protocol addresses, and what the
    /// wrapper delegates to.
    fn name(&self) -> &syn::Ident {
        &self.sig.ident
    }
}

/// An operation's parameter its tail fills, whatever its role.
struct TailArg {
    ident: syn::Ident,
    /// `Scoped<T>` or `Ambient<T>`, as written.
    ty: Type,
    kind: TailParam,
}

/// A `#[tool]` / `#[prompt]` parameter whose wire value goes through a pipe:
/// `Parameters<Piped<P>>`.
struct PipedArg {
    ident: syn::Ident,
    /// The carrier as the method spells it, inside `Parameters`.
    carrier: Type,
    /// The pipe's `In` — what the wire carries and what rmcp derives the
    /// schema from.
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
    let markers = declared
        .iter()
        .map(|keys| keys.markers(&self_ty, &item.generics));
    let markers = quote!(#(#markers)*);
    if operations.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[tools] on an impl with no #[tool], #[prompt], #[resource] or #[resources] \
             method has nothing to mount — drop the decorator, or mark the methods it \
             should serve",
        ));
    }
    if let (Some(listing), true) = (operations.listings.first(), operations.resources.is_empty()) {
        return Err(syn::Error::new_spanned(
            listing.name(),
            "a #[resources] listing lists the URIs a #[resource] reads, and this #[tools] \
             block declares none — declare the `#[resource(template = \"…\")]` that reads \
             what it lists",
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
    let resources = resources_impl(&self_ty, &generics, &operations)?;
    let resource_methods = resource_handler_methods(&operations);

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
        let resources = surface(&operations.resources, quote!(resources));
        let listings = surface(&operations.listings, quote!(resources));
        quote! {{
            #[allow(unused_mut)]
            let mut __capabilities = ServerCapabilities::builder().build();
            #(#tools)*
            #(#prompts)*
            #(#resources)*
            #(#listings)*
            __capabilities
        }}
    };

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
    let templates = template_items(&operations);

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

            #templates
        }

        impl #impl_generics ::nest_rs_mcp::__private::ToolsHost for #self_ty #ty_generics
            #where_clause
        {
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
            #resources

            #handler_attrs
            impl #impl_generics ServerHandler for #self_ty #ty_generics #where_clause {
                fn get_info(&self) -> ServerConfig {
                    ServerConfig::new(#capabilities)
                }

                #resource_methods
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

/// The wrappers the generated `read_resource` and `list_resources` call, one
/// per `#[resource]` and `#[resources]` method, in a plain impl: rmcp routes
/// neither.
fn resources_impl(
    self_ty: &Type,
    (impl_generics, ty_generics, where_clause): &Generics<'_>,
    operations: &Operations,
) -> syn::Result<Option<TokenStream2>> {
    let wrappers = operations
        .resources
        .iter()
        .chain(&operations.listings)
        .map(|op| resource_wrapper(self_ty, op))
        .collect::<syn::Result<Vec<_>>>()?;
    if wrappers.is_empty() {
        return Ok(None);
    }
    Ok(Some(quote! {
        impl #impl_generics #self_ty #ty_generics #where_clause {
            #(#wrappers)*
        }
    }))
}

/// The `ServerHandler` methods serving the resource roles: the listing, the
/// templates and the read, which offers a URI to each template in
/// declaration order — none reading it is `resource_not_found`.
fn resource_handler_methods(operations: &Operations) -> TokenStream2 {
    let listings = operations.listings.iter().map(|op| {
        let cfgs = &op.cfgs;
        let wrapper = &op.wrapper;
        quote! {
            #(#cfgs)*
            async fn list_resources(
                &self,
                _request: ::core::option::Option<::nest_rs_mcp::model::PaginatedRequestParams>,
                _context: ::nest_rs_mcp::service::RequestContext<
                    ::nest_rs_mcp::service::RoleServer,
                >,
            ) -> ::core::result::Result<
                ::nest_rs_mcp::model::ListResourcesResult,
                ::nest_rs_mcp::McpError,
            > {
                self.#wrapper().await
            }
        }
    });
    if operations.resources.is_empty() {
        return quote!(#(#listings)*);
    }
    let arms = operations.resources.iter().filter_map(|op| {
        let decl = op.resource.as_ref()?;
        let template = site_const_ident("template", op.name());
        Some(resource::read_arm(decl, &template, &op.wrapper, &op.cfgs))
    });
    quote! {
        #(#listings)*

        async fn list_resource_templates(
            &self,
            _request: ::core::option::Option<::nest_rs_mcp::model::PaginatedRequestParams>,
            _context: ::nest_rs_mcp::service::RequestContext<::nest_rs_mcp::service::RoleServer>,
        ) -> ::core::result::Result<
            ::nest_rs_mcp::model::ListResourceTemplatesResult,
            ::nest_rs_mcp::McpError,
        > {
            ::core::result::Result::Ok(
                ::nest_rs_mcp::model::ListResourceTemplatesResult::with_all_items(
                    <Self>::__nestrs_mcp_resource_templates(),
                ),
            )
        }

        async fn read_resource(
            &self,
            request: ::nest_rs_mcp::model::ReadResourceRequestParams,
            _context: ::nest_rs_mcp::service::RequestContext<::nest_rs_mcp::service::RoleServer>,
        ) -> ::core::result::Result<
            ::nest_rs_mcp::model::ReadResourceResponse,
            ::nest_rs_mcp::McpError,
        > {
            let __nestrs_uri = request.uri;
            #(#arms)*
            ::core::result::Result::Err(::nest_rs_mcp::__private::resource_not_found())
        }
    }
}

/// Each `#[resource]`'s template, as a const of the host, and the templates
/// `resources/templates/list` advertises and the boot checks.
fn template_items(operations: &Operations) -> TokenStream2 {
    if operations.resources.is_empty() {
        return quote!();
    }
    let consts = operations.resources.iter().filter_map(|op| {
        let decl = op.resource.as_ref()?;
        let cfgs = &op.cfgs;
        let name = site_const_ident("template", op.name());
        let template = resource::template_const(decl);
        Some(quote! {
            #(#cfgs)*
            #[doc(hidden)]
            const #name: ::nest_rs_mcp::__private::UriTemplate = #template;
        })
    });
    let advertised = operations.resources.iter().filter_map(|op| {
        let decl = op.resource.as_ref()?;
        let cfgs = &op.cfgs;
        let template = resource::advertised(decl, &op.sig.output);
        Some(quote! {
            #(#cfgs)*
            __nestrs_templates.push(#template);
        })
    });
    quote! {
        #(#consts)*

        /// The templates the host's `#[resource]` methods read through, in
        /// declaration order.
        #[doc(hidden)]
        pub fn __nestrs_mcp_resource_templates()
            -> ::std::vec::Vec<::nest_rs_mcp::model::ResourceTemplate>
        {
            #[allow(unused_mut, reason = "every resource may be compiled out")]
            let mut __nestrs_templates = ::std::vec::Vec::new();
            #(#advertised)*
            __nestrs_templates
        }
    }
}

/// The generated method a resource role is served through: the operation
/// dispatched through its site, the authored method its tail, its answer read
/// by its type — a `#[resource]`'s into the contents of the URI it was read
/// at, the `#[resources]` listing's into the listing.
fn resource_wrapper(self_ty: &Type, op: &Operation) -> syn::Result<TokenStream2> {
    let Operation {
        role,
        wrapper,
        sig,
        posture,
        cfgs,
        ..
    } = op;
    let variables = op
        .resource
        .iter()
        .flat_map(|decl| &decl.variables)
        .map(|arg| {
            let (ident, ty) = (&arg.ident, &arg.ty);
            quote!(#ident: #ty)
        });
    // At the return type, so a value that is no resource's answer is refused there.
    let into = role.wire_answer();
    let (params, output, answered) = if *role == Role::Resource {
        let masked = masked(*role, posture, sig)?;
        let read = quote_spanned! {answer_span(sig)=>
            #into::into_read(__nestrs_value, __nestrs_uri)
        };
        (
            quote!(__nestrs_uri: ::std::string::String, #(#variables),*),
            quote!(::nest_rs_mcp::model::ReadResourceResult),
            quote!((#masked).and_then(|__nestrs_value| #read)),
        )
    } else {
        let listed = quote_spanned! {answer_span(sig)=> #into::into_listing(__nestrs_value)};
        (
            quote!(),
            quote!(::nest_rs_mcp::model::ListResourcesResult),
            quote!(::core::result::Result::Ok(#listed)),
        )
    };
    let validate_prelude = op
        .resource
        .iter()
        .flat_map(|decl| &decl.variables)
        .map(|arg| {
            let ident = &arg.ident;
            validate_statement(&quote!(&#ident))
        });
    let body = dispatch(self_ty, op, quote!(#(#validate_prelude)*), answered)?;
    Ok(quote! {
        #(#cfgs)*
        async fn #wrapper(&self, #params)
            -> ::core::result::Result<#output, ::nest_rs_mcp::McpError>
        {
            #body
        }
    })
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
        tail,
        ..
    } = op;
    let name = op.name();

    // Each pipe carrier is replaced by the value it wraps, so rmcp derives the
    // tool's JSON Schema from `T`; a `Scoped<T>` or an `Ambient<T>` is the tail's,
    // never rmcp's.
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
                if tail.iter().any(|arg| arg.ident == pat_ident.ident) {
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
    let validate_prelude = parameters_args(sig)
        .into_iter()
        .map(|ident| validate_statement(&quote!(&#ident.0)));
    let body = dispatch(
        self_ty,
        op,
        quote!(#(#validate_prelude)* #(#pipe_prelude)*),
        masked(*role, posture, sig)?,
    )?;
    Ok(quote! {
        #role_attr
        #wire_sig {
            #body
        }
    })
}

/// Every wrapper's body: the operation dispatched through its site, its tail
/// running `prelude`, the `Scoped<T>` and `Ambient<T>` parameters and the authored method,
/// whose value `answered` turns into what the wrapper returns.
fn dispatch(
    self_ty: &Type,
    op: &Operation,
    prelude: TokenStream2,
    answered: TokenStream2,
) -> syn::Result<TokenStream2> {
    let Operation {
        role,
        sig,
        tail,
        wire_name,
        ..
    } = op;
    let name = op.name();
    let tail_prelude = tail.iter().map(|TailArg { ident, ty, kind }| match kind {
        TailParam::Scoped => quote! { let #ident: #ty = ::nest_rs_mcp::__private::scoped()?; },
        TailParam::Ambient => {
            quote! { let #ident: #ty = ::nest_rs_mcp::__private::ambient_parameter()?; }
        }
    });
    let call_args = forwarded_arg_idents(sig)?;
    let call = await_if_async(sig, quote!(<#self_ty>::#name(self, #(#call_args),*)));
    // What the method answers is read by its type, refused at its return type.
    let returned_by = quote_spanned!(answer_span(sig)=> __nestrs_answered);
    let answer = quote_spanned! {answer_span(sig)=>
        ::nest_rs_mcp::__private::answer_of(#returned_by)
    };
    let site = site_ref(self_ty, name);
    let kind = role.kind();
    let wire_name = LitStr::new(wire_name, name.span());
    Ok(quote! {
        ::nest_rs_mcp::__private::run_operation(
            #site,
            ::nest_rs_mcp::__private::operation_view(
                #kind,
                #wire_name,
                ::core::any::type_name::<#self_ty>(),
            ),
            move || async move {
                #prelude
                #(#tail_prelude)*
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
    let PipedArg { ident, carrier, .. } = arg;
    quote! {
        let ::nest_rs_mcp::Parameters(__nestrs_value) = #ident;
        let #ident = ::nest_rs_mcp::Parameters(
            <#carrier>::apply(__nestrs_value)
                .map_err(::nest_rs_mcp::__private::OperationFailure::from)?,
        );
    }
}

/// Step 3, first — hold a decoded argument, `argument` borrowing it, to the
/// rules its type declares, before a pipe runs over it.
fn validate_statement(argument: &TokenStream2) -> TokenStream2 {
    let check = validate_argument(&quote!(::nest_rs_mcp), argument);
    quote! {
        #check.map_err(::nest_rs_mcp::__private::OperationFailure::from)?;
    }
}

/// Every `Parameters<…>` argument of `sig`, the arguments a tool or a prompt
/// decodes, by the ident the normalized signature binds.
fn parameters_args(sig: &Signature) -> Vec<&syn::Ident> {
    sig.inputs
        .iter()
        .filter_map(|input| {
            let FnArg::Typed(pat_type) = input else {
                return None;
            };
            let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
                return None;
            };
            let (ident, _) = generic_args(&pat_type.ty)?;
            (ident == "Parameters").then_some(&pat_ident.ident)
        })
        .collect()
}

/// Step 4 — the answer's value, `__nestrs_value`, with response masking when
/// the posture arms it.
///
/// `Json<T>` is unwrapped before masking and rewrapped after: rmcp's wrapper is
/// neither `Serialize` nor `DeserializeOwned`. A resource's `Option<Json<T>>`
/// is masked inside the `Option`, `None` passing through. `take_operation`
/// already refused a masked role whose every answer is opaque.
fn masked(role: Role, posture: &Posture, sig: &Signature) -> syn::Result<TokenStream2> {
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
    // Opaque content: refused here, or it fails deep inside the expansion or,
    // compiled, every call closed.
    if let Some(opaque) = opaque_answer(role, ok_ty) {
        let remedy = match role {
            Role::Resource => role.mask_unwraps(),
            Role::Tool | Role::Prompt | Role::Resources => "`Json<T>` or `T`",
        };
        return Err(syn::Error::new_spanned(
            &sig.output,
            format!(
                "`#[authorize(...)]` cannot mask a `{opaque}` — its contents are opaque to \
                 the entity round-trip. Return the wire type ({remedy}) and let the \
                 mask run, or keep the gate and mask by hand with \
                 `#[authorize(Action, Entity, unmasked)]` + `nest_rs::authz::masked_reply`",
            ),
        ));
    }

    if let Some(inner) = nth_generic_type(ok_ty, "Option", 0)
        .and_then(|optional| nth_generic_type(optional, "Json", 0))
    {
        return Ok(quote! {
            __nestrs_value
                .map(|::nest_rs_mcp::Json(__nestrs_out)| {
                    ::nest_rs_authz::__private::mcp_masked_value::<#action, #entity, #inner>(__nestrs_out)
                        .map(::nest_rs_mcp::Json)
                })
                .transpose()
                .map_err(::nest_rs_mcp::__private::OperationFailure::from)
        });
    }

    if let Some(inner) = nth_generic_type(ok_ty, "Json", 0) {
        return Ok(quote! {{
            let ::nest_rs_mcp::Json(__nestrs_out) = __nestrs_value;
            ::nest_rs_authz::__private::mcp_masked_value::<#action, #entity, #inner>(__nestrs_out)
                .map(::nest_rs_mcp::Json)
                .map_err(::nest_rs_mcp::__private::OperationFailure::from)
        }});
    }

    Ok(quote! {
        ::nest_rs_authz::__private::mcp_masked_value::<#action, #entity, _>(__nestrs_value)
            .map_err(::nest_rs_mcp::__private::OperationFailure::from)
    })
}

/// The opaque content type `role` answers in `ok_ty`, a resource's inside its
/// `Option` too, which `None` passing through leaves to the mask.
fn opaque_answer(role: Role, ok_ty: &Type) -> Option<&syn::Ident> {
    let answered = match role {
        Role::Resource => nth_generic_type(ok_ty, "Option", 0).unwrap_or(ok_ty),
        Role::Tool | Role::Prompt | Role::Resources => ok_ty,
    };
    let Type::Path(path) = answered else {
        return None;
    };
    let last = &path.path.segments.last()?.ident;
    role.opaque_answers()
        .iter()
        .any(|opaque| last == opaque)
        .then_some(last)
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
) -> syn::Result<(Operations, [DispatchKeys; 3])> {
    let mut operations = Operations::default();
    // rmcp's router keeps the last route added under a name, silently.
    let mut declared = DispatchKeys::new(
        "#[tools]",
        "a host routes each tool name, and each prompt name, to one method, so one of the \
         two would never run — give one a distinct `name`",
    );
    // `read_resource` offers a URI to the templates in order: the second of
    // two equal ones would never read one.
    let mut templates = DispatchKeys::new(
        "#[tools]",
        "a host reads the URIs one template writes through one method, so one of the two \
         would never run — give one a distinct `template`",
    );
    let mut listings = DispatchKeys::new(
        "#[tools]",
        "`resources/list` is answered by one method per host — list every resource there",
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
        let cfgs = cfg_attrs(&method.attrs);
        let (wire_name, resource) = match role {
            Role::Tool | Role::Prompt => {
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
                    &cfgs,
                    role_attr,
                )?;
                (wire_name, None)
            }
            Role::Resource => {
                let decl = resource::declaration(method, index)?;
                let wire_name = decl.template.value();
                templates.declare(
                    Collision::Marker,
                    role.label(),
                    &wire_name,
                    &format!("#[resource(template = {wire_name:?})]"),
                    &method.sig.ident,
                    &cfgs,
                    role_attr,
                )?;
                (wire_name, Some(decl))
            }
            Role::Resources => {
                resource::listing_declaration(role_attr)?;
                listings.declare(
                    Collision::Marker,
                    role.label(),
                    "resources/list",
                    "#[resources]",
                    &method.sig.ident,
                    &cfgs,
                    role_attr,
                )?;
                (method.sig.ident.to_string(), None)
            }
        };

        let operation = take_operation(method, index, role, base, wire_name, resource)?;
        match role {
            Role::Tool => operations.tools.push(operation),
            Role::Prompt => operations.prompts.push(operation),
            Role::Resource => operations.resources.push(operation),
            Role::Resources => operations.listings.push(operation),
        }
    }

    Ok((operations, [declared, templates, listings]))
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
    mut resource: Option<ResourceDecl>,
) -> syn::Result<Operation> {
    let name = method.sig.ident.clone();
    let (role_attr, description_check) = if role.routed_by_rmcp() {
        wrapper_role_attr(method, index, role)?
    } else {
        let check = resource
            .as_mut()
            .and_then(|decl| decl.description_check.take());
        (quote!(), check)
    };
    // Left on the authored method, rmcp would route it as a second tool of the
    // same name, and rustc would refuse a resource role as an unknown attribute.
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
    if posture.masks()
        && let Some(why) = role.unmaskable()
    {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            format!(
                "`#[authorize(...)]` cannot mask a {why}: \
                 `#[authorize(Action, Entity, unmasked)]`"
            ),
        ));
    }
    if posture.masks() && result_ok_type(&method.sig).is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig.output,
            format!(
                "a masked {} spells its return `Result<…>`: the mask reads the \
                 value's shape — {} to unwrap, `{}` to refuse — off \
                 that spelling, and a `Result` renamed on import or behind an alias hides \
                 it. Write `Result<…>`, or declare `#[authorize(Action, Entity, unmasked)]` \
                 and mask in the body",
                role.attr(),
                role.mask_unwraps(),
                role.opaque_answers().first().copied().unwrap_or_default(),
            ),
        ));
    }

    let mut sig = method.sig.clone();
    normalize_forwarded_args(sig.inputs.iter_mut())?;
    let pipes = match role {
        Role::Tool | Role::Prompt => piped_args(&sig)?,
        Role::Resource | Role::Resources => Vec::new(),
    };
    match (role, resource.as_mut()) {
        (Role::Resource, Some(decl)) => resource::bind_variables(decl, &sig)?,
        (Role::Resources, _) => resource::refuse_listing_arguments(&sig)?,
        _ => {}
    }
    let tail = tail_args(&sig);

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
        tail,
        cfgs,
        wire_name,
        metas,
        resource,
    })
}

/// Every `Scoped<T>` and `Ambient<T>` parameter, read by its last segment: the
/// tail resolves it once the guards and the gate admitted the caller, so the
/// wire signature rmcp extracts from leaves it out.
fn tail_args(sig: &Signature) -> Vec<TailArg> {
    sig.inputs
        .iter()
        .filter_map(|input| {
            let FnArg::Typed(pat_type) = input else {
                return None;
            };
            let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
                return None;
            };
            tail_param(&pat_type.ty).map(|kind| TailArg {
                ident: pat_ident.ident.clone(),
                ty: (*pat_type.ty).clone(),
                kind,
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
        op.tail
            .iter()
            .filter(|arg| arg.kind == TailParam::Scoped)
            .map(move |TailArg { ty, .. }| {
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
    let description_check = description_check(description, method)?;
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
pub(crate) enum Description {
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
    pub(crate) fn read(doc: TokenStream2) -> Self {
        match syn::parse2::<LitStr>(doc.clone()) {
            Ok(_) => Self::Read,
            Err(_) => Self::Evaluated(doc),
        }
    }

    /// A stated `description = …` value, read as far as the compiler can read
    /// it. A blank literal is refused at the literal.
    pub(crate) fn stated(value: &syn::Expr) -> syn::Result<Self> {
        Ok(match ungrouped_expr(value) {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) => {
                if text.value().trim().is_empty() {
                    return Err(syn::Error::new_spanned(text, NEEDS_A_DESCRIPTION));
                }
                Self::Read
            }
            expr @ (syn::Expr::Macro(_) | syn::Expr::Path(_)) => Self::Evaluated(quote!(#expr)),
            _ => Self::Runtime,
        })
    }
}

/// The `const` refusing a description only the compiler can read blank, at
/// `method`; no description at all is refused there.
pub(crate) fn description_check(
    description: Option<Description>,
    method: &ImplItemFn,
) -> syn::Result<Option<TokenStream2>> {
    match description {
        None => Err(syn::Error::new_spanned(
            &method.sig.ident,
            NEEDS_A_DESCRIPTION,
        )),
        Some(Description::Read | Description::Runtime) => Ok(None),
        Some(Description::Evaluated(expr)) => {
            let span = method.sig.ident.span();
            Ok(Some(quote_spanned! {span=>
                const _: () = ::core::assert!(
                    !::nest_rs_mcp::__private::description_is_blank(#expr),
                    #NEEDS_A_DESCRIPTION,
                );
            }))
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
    Description::stated(value).map(Some)
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
pub(crate) fn doc_comment(attrs: &[Attribute]) -> Option<TokenStream2> {
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
        bind_unsupported_because: match role {
            Role::Tool | Role::Prompt => {
                "an operation takes one `Parameters<T>` struct, not the named id argument the \
                 binding reads. Keep `#[authorize(Action, Entity)]` and bind in the body with \
                 the service's `access`"
            }
            Role::Resource => {
                "a resource reads its row in the body, off its template's variables, and \
                 answers `None` for one the caller may not read. Keep \
                 `#[authorize(Action, Entity)]` and load it with the service's `access`"
            }
            Role::Resources => "a listing loads no one row to bind",
        },
    }
}

/// Every `Parameters<Piped<P>>` argument, matched on the normalized signature
/// so each carries the ident the wrapper declares.
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
        if piped_input(&pat_type.ty).is_some() {
            return Err(syn::Error::new_spanned(
                &pat_type.ty,
                "a pipe on an MCP operation wraps the *arguments*, so it goes inside \
                 `Parameters<…>` — write `Parameters<Piped<P>>`, which exposes the \
                 pipe's input as the operation's schema",
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
        let Some(value_ty) = piped_input(&carrier) else {
            continue;
        };
        args.push(PipedArg {
            ident: pat_ident.ident.clone(),
            value_ty,
            carrier,
        });
    }
    Ok(args)
}

/// An item `#[tools]` refuses, spanned on the item itself.
fn unsupported(entry: &ImplItem) -> syn::Error {
    syn::Error::new_spanned(
        entry,
        "#[tools] serves only #[tool], #[prompt], #[resource] and #[resources] methods — \
         move anything else to a plain `impl` block beside it",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque(role: Role, ok: &str) -> Option<String> {
        let ty: Type = syn::parse_str(ok).expect("a type");
        opaque_answer(role, &ty).map(ToString::to_string)
    }

    #[test]
    fn every_opaque_content_type_a_tool_answers_is_found_by_its_last_segment() {
        for ok in [
            "CallToolResult",
            "rmcp::model::CallToolResponse",
            "ContentBlock",
            "nest_rs::mcp::model::InputRequiredResult",
        ] {
            assert!(opaque(Role::Tool, ok).is_some(), "{ok}");
        }
        for ok in ["Json<Post>", "String", "Vec<Post>", "ReadResourceResult"] {
            assert_eq!(opaque(Role::Tool, ok), None, "{ok}");
        }
    }

    #[test]
    fn a_resource_s_read_result_is_opaque_inside_its_option_too() {
        for ok in ["ReadResourceResult", "Option<model::ReadResourceResult>"] {
            assert_eq!(
                opaque(Role::Resource, ok).as_deref(),
                Some("ReadResourceResult"),
                "{ok}",
            );
        }
        for ok in [
            "Json<Post>",
            "Option<Json<Post>>",
            "String",
            "CallToolResult",
        ] {
            assert_eq!(opaque(Role::Resource, ok), None, "{ok}");
        }
    }

    #[test]
    fn a_role_whose_every_answer_is_opaque_is_refused_whole_rather_than_by_type() {
        for role in [Role::Prompt, Role::Resources] {
            assert!(role.unmaskable().is_some(), "{}", role.attr());
            assert!(role.opaque_answers().is_empty(), "{}", role.attr());
        }
        for role in [Role::Tool, Role::Resource] {
            assert_eq!(role.unmaskable(), None, "{}", role.attr());
        }
    }
}
