use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Expr, ItemStruct, LitStr};

use nest_rs_codegen::{
    Edge, InjectableBody, build_injectable_body, from_container_method, guard_capability_bounds,
    injected_keys_with_layers, injected_names_with_layers, layer_deps, reject_http_only_layers,
    scoped_specs, take_path_list,
};

pub(crate) fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    match pair::MCP.parse_host(input.into()) {
        Ok(item) => mcp_struct(args, item),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `#[tools]` — the operations half, on the host's inherent impl. Named for
/// what it collects, the way `#[routes]` and `#[messages]` are; it carries the
/// `#[prompt]` methods too, since a prompt is an operation this same host
/// serves and rmcp routes both through one `ServerHandler`.
pub(crate) fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = match pair::MCP.parse_operations(input.into()) {
        Ok(item) => crate::mcp_impl::mcp_impl(args, item),
        Err(err) => err.to_compile_error().into(),
    };
    // The struct half holds the host to `McpHost`, which `ServerHandler` answers,
    // so a refused block still names one — every method at its default — rather
    // than add an unsatisfied bound to the refusal.
    pair::MCP
        .keep_item_on_refusal(written, expansion.into(), &["tool", "prompt"], |item| {
            let self_ty = &item.self_ty;
            let (impl_generics, _, where_clause) = item.generics.split_for_impl();
            quote! {
                impl #impl_generics ::nest_rs_mcp::ServerHandler for #self_ty #where_clause {}
            }
        })
        .into()
}

fn mcp_struct(args: TokenStream, mut item: ItemStruct) -> TokenStream {
    let args = match parse_mcp_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return refused_beside(err, item),
    };

    // Interceptors and filters have no per-operation seam on this transport, so
    // binding one here would be a silent no-op — named compile error instead,
    // the same answer GraphQL and WS give.
    if let Err(err) = reject_http_only_layers(&item.attrs, "MCP", "host") {
        return refused_beside(err, item);
    }
    // Host-scope (provider) guard declarations — same shape and same mental
    // model as `#[controller] struct` + `#[resolver] struct` + `#[gateway]
    // struct`. Stored here so the impl-form macro folds them into every
    // operation's chain at runtime through `__nestrs_mcp_host_guard_specs()`.
    let guards = match take_path_list(&mut item.attrs, "use_guards") {
        Ok(paths) => paths,
        Err(err) => return refused_beside(err, item),
    };
    // Host-scope guards fold into the same per-operation chain the operations'
    // own do, so they run `Guard::check_mcp` and owe the same capability.
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::McpGuard));

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        ..
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return refused_beside(err, item),
    };

    let name = item.ident.clone();
    let host_name = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The struct's `#[inject]` keys + host-scope guards + whatever the decorated
    // impl declared per operation. The last part is why this reads through
    // `__nestrs_mcp_operation_layers()`: `#[mcp]` on the struct is what emits
    // `Discoverable` (a host may serve a hand-written `ServerHandler` and have no
    // decorated impl at all), so the per-operation guards have to arrive from the
    // other half rather than be folded in here.
    let layers = layer_deps(guards.iter());
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));

    // Empty stands for "the host declared none": the crate substitutes its
    // default endpoint. The decorator keeps no default of its own, so the path
    // a host lands on has one home and cannot drift between the two crates.
    let path = args.path.unwrap_or_else(|| LitStr::new("", name.span()));
    let (identity_name, identity_title) =
        (opt_str(args.name.as_ref()), opt_str(args.title.as_ref()));

    let residency = pair::MCP.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container

            /// Host-scope `#[use_guards(...)]`, read by the impl-form macro on
            /// the cache miss that composes an operation's chain. Empty when
            /// none declared.
            #[doc(hidden)]
            pub fn __nestrs_mcp_host_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                #guard_specs
            }
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
                // An inherent associated fn wins over a trait one, so this is
                // the decorated impl's own list when there is one, and the
                // fallback's empty pair when the host writes rmcp by hand.
                use ::nest_rs_mcp::DefaultOperationLayers as _;
                let mut __keys: ::std::vec::Vec<::core::any::TypeId> = #injected_keys;
                __keys.extend(<Self>::__nestrs_mcp_operation_layers().0);
                __keys
            }

            fn injected_names() -> ::std::vec::Vec<&'static str> {
                use ::nest_rs_mcp::DefaultOperationLayers as _;
                let mut __names: ::std::vec::Vec<&'static str> = #injected_names;
                __names.extend(<Self>::__nestrs_mcp_operation_layers().1);
                __names
            }

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                // Contribute to the endpoint at `path` — the *first* host on a
                // path attaches the mount, every host attaches itself. The
                // default path, the grouping, the merge, the guard/context/
                // config resolution, the identity overlay and the duplicate-tool
                // boot check all live in the crate, so the mount policy is
                // testable rather than macro-expanded.
                ::nest_rs_mcp::register_host::<Self>(
                    builder,
                    #path,
                    #host_name,
                    ::nest_rs_mcp::McpIdentity::declared(#identity_name, #identity_title),
                    |__c| -> ::std::sync::Arc<dyn ::nest_rs_mcp::McpHost> {
                        ::std::sync::Arc::new(<Self>::from_container(__c))
                    },
                    || {
                        // An inherent associated fn wins over a trait one, so
                        // this is the host's real router — whether the impl-level
                        // `#[mcp]` emitted it (as `pub(crate)`, so it is nameable
                        // from here) or the host wrote rmcp's `#[tool_router]`
                        // itself — and an empty stand-in when it has neither.
                        // The boot check that catches a duplicate tool name is
                        // only as good as this list.
                        use ::nest_rs_mcp::DefaultToolRouter as _;
                        <Self>::tool_router().list_all()
                    },
                )
            }
        }
    }
    .into()
}

/// A refused `#[mcp(..)]` argument, reported **beside the struct as written**.
///
/// Returning the error alone dropped the struct, so the `#[tools]` impl beside
/// it answered `cannot find type` — a second error blamed on correct code, and
/// the one host of the four that cascaded. The helper attributes this decorator
/// would have consumed come off, or rustc would report them as unknown, and the
/// one item the impl half reads off the host is filled in empty — the mirror of
/// what `DecoratorPair::keep_item_on_refusal` does for an impl half. The
/// refusal already fails the build; the stand-in only keeps it the one error.
fn refused_beside(err: syn::Error, mut item: ItemStruct) -> TokenStream {
    item.attrs.retain(|attr| {
        ![
            "use_guards",
            "use_interceptors",
            "use_filters",
            "use_exception_filters",
        ]
        .iter()
        .any(|consumed| attr.path().is_ident(consumed))
    });
    if let syn::Fields::Named(fields) = &mut item.fields {
        for field in &mut fields.named {
            field.attrs.retain(|attr| !attr.path().is_ident("inject"));
        }
    }
    let err = err.to_compile_error();
    let name = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    quote! {
        #err
        #item
        impl #impl_generics #name #ty_generics #where_clause {
            #[doc(hidden)]
            pub fn __nestrs_mcp_host_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                ::std::vec::Vec::new()
            }
        }
    }
    .into()
}

/// Everything `#[mcp(..)]` accepts. Every argument is optional: a bare `#[mcp]`
/// serves the default endpoint and lets the app's identity speak for it.
///
/// `path` is a literal — it is a route, and the same shape `#[controller]`
/// takes. The identity arguments stay whole expressions so a host can name
/// itself from its own build environment (`name = env!("CARGO_PKG_NAME")`);
/// each is passed to `McpIdentity::declared`, whose `Option<&str>` parameters
/// are what reject anything else, spanned on the offending expression.
///
/// The pair is what a host can honestly say: *which endpoint stands apart*. The
/// server's own `version` is not on that list — a feature library knows neither
/// the binary's version nor, on a shared endpoint, the whole surface — so it is
/// declared once by the app, through `McpModule::for_root`. Nor is any other
/// field of the identity: `nest_rs_codegen::MCP_GRAMMAR` refuses each of them by
/// name and points at that same seam, because a key that exists and is
/// somebody's deserves an answer rather than a list of spellings.
///
/// Unlike a controller's, this path is not a namespace the host owns — nothing
/// nests under it. It names the one endpoint the host joins, which is why
/// peers share it verbatim.
#[derive(Default)]
struct McpArgs {
    path: Option<LitStr>,
    name: Option<Expr>,
    title: Option<Expr>,
}

fn parse_mcp_args(args: TokenStream2) -> syn::Result<McpArgs> {
    // Before this decorator's own unknown-key arm, because `version` is not a
    // typo here: it is the word a developer carries over from `#[controller(
    // version = "1")]`, where it selects an address. MCP's answer to that — the
    // path is the address, the server's version is the app's one declaration —
    // is worded once, in `nest-rs-codegen`, for every edge that refuses it.
    Edge::Mcp.reject_version(&args)?;
    let mut parsed = McpArgs::default();
    // Accepting a repeat drops one of two declarations and source order decides
    // which — here that is the path a host joins, i.e. which peers share its
    // endpoint, and the identity a client is told it reached.
    nest_rs_codegen::MCP_GRAMMAR.parse2(args, |arg| {
        match arg.key() {
            "path" => parsed.path = Some(arg.str_lit("/mcp")?),
            "name" => parsed.name = Some(arg.expr()?),
            "title" => parsed.title = Some(arg.expr()?),
            // The grammar hands over only its own keys.
            _ => {}
        }
        Ok(())
    })?;
    if let Some(path) = &parsed.path {
        check_path(path)?;
    }
    Ok(parsed)
}

/// A host's `path` is the whole URL path a client is configured with. Written
/// empty it says nothing at all, and the argument that says nothing is the
/// absent one — two spellings for one mount is what the framework does not
/// ship.
fn check_path(path: &LitStr) -> syn::Result<()> {
    // The shared grammar refuses the empty string too, in the same words as its
    // two siblings — this used to be the only one of three `path` keys that
    // checked anything, and it checked one of the four ways of getting it wrong.
    nest_rs_codegen::reject_path("mcp", path)
}

/// An optional identity argument as the `Option<&str>` tokens
/// `McpIdentity::declared` takes.
fn opt_str(expr: Option<&Expr>) -> TokenStream2 {
    match expr {
        Some(value) => quote! { ::core::option::Option::Some(#value) },
        None => quote! { ::core::option::Option::None },
    }
}
