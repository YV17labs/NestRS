use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, ItemStruct, LitStr, Meta, Token};

use nest_rs_codegen::{
    DecoratorPair, InjectableBody, build_injectable_body, expr_str, from_container_method,
    guard_capability_bounds, injected_keys_with_layers, injected_names_with_layers, layer_deps,
    reject_http_only_layers, scoped_specs, take_path_list,
};

/// The MCP edge's pair. Naming the sibling is the whole point of the split: the
/// shape the developer reached for exists, it is just spelled with the other
/// decorator — and `parse_host` / `parse_operations` parse the item *before*
/// complaining, which is what keeps a broken `impl` from being reported as
/// "expected struct".
pub(crate) const MCP_PAIR: DecoratorPair = DecoratorPair {
    host: "#[mcp]",
    subject: "host struct",
    operations: "#[tools]",
    collects: "#[tool] / #[prompt]",
};

pub(crate) fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    match MCP_PAIR.parse_host(input.into()) {
        Ok(item) => mcp_struct(args, item),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `#[tools]` — the operations half, on the host's inherent impl. Named for
/// what it collects, the way `#[routes]` and `#[messages]` are; it carries the
/// `#[prompt]` methods too, since a prompt is an operation this same host
/// serves and rmcp routes both through one `ServerHandler`.
pub(crate) fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    match MCP_PAIR.parse_operations(input.into()) {
        Ok(item) => crate::mcp_impl::mcp_impl(args, item),
        Err(err) => err.to_compile_error().into(),
    }
}

fn mcp_struct(args: TokenStream, mut item: ItemStruct) -> TokenStream {
    let args = match parse_mcp_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };

    // Interceptors and filters have no per-operation seam on this transport, so
    // binding one here would be a silent no-op — named compile error instead,
    // the same answer GraphQL and WS give.
    if let Err(err) = reject_http_only_layers(&item.attrs, "MCP", "host") {
        return err.to_compile_error().into();
    }
    // Host-scope (provider) guard declarations — same shape and same mental
    // model as `#[controller] struct` + `#[resolver] struct` + `#[gateway]
    // struct`. Stored here so the impl-form macro folds them into every
    // operation's chain at runtime through `__nestrs_mcp_host_guard_specs()`.
    let guards = match take_path_list(&mut item.attrs, "use_guards", "guard") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
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
        Err(err) => return err.to_compile_error().into(),
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
    let (identity_name, identity_version, identity_title) = (
        opt_str(args.name.as_ref()),
        opt_str(args.version.as_ref()),
        opt_str(args.title.as_ref()),
    );

    quote! {
        #item

        #capability_bounds

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
                    ::nest_rs_mcp::McpIdentity::declared(
                        #identity_name,
                        #identity_version,
                        #identity_title,
                    ),
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

/// Everything `#[mcp(..)]` accepts. Every argument is optional: a bare `#[mcp]`
/// serves the default endpoint and lets the app's identity speak for it.
///
/// `path` is a literal — it is a route, and the same shape `#[controller]`
/// takes. The identity arguments stay whole expressions so an app-owned host
/// can write `version = env!("CARGO_PKG_VERSION")`; each is passed to
/// `McpIdentity::declared`, whose `Option<&str>` parameters are what reject
/// anything else, spanned on the offending expression.
///
/// Unlike a controller's, this path is not a namespace the host owns — nothing
/// nests under it. It names the one endpoint the host joins, which is why
/// peers share it verbatim.
#[derive(Default)]
struct McpArgs {
    path: Option<LitStr>,
    name: Option<Expr>,
    version: Option<Expr>,
    title: Option<Expr>,
}

fn parse_mcp_args(args: TokenStream2) -> syn::Result<McpArgs> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut parsed = McpArgs::default();
    for meta in metas {
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => {
                parsed.path = Some(expr_str(&nv.value)?)
            }
            Meta::NameValue(nv) if nv.path.is_ident("name") => parsed.name = Some(nv.value),
            Meta::NameValue(nv) if nv.path.is_ident("version") => parsed.version = Some(nv.value),
            Meta::NameValue(nv) if nv.path.is_ident("title") => parsed.title = Some(nv.value),
            // `instructions` deliberately absent: it describes the *server*, so
            // it is the app's one declaration
            // (`McpOptions { server: McpIdentity::new(..).instructions(..) }`).
            // A host writing it would speak for peers it cannot see.
            Meta::NameValue(nv) if nv.path.is_ident("instructions") => {
                return Err(syn::Error::new_spanned(
                    nv,
                    "#[mcp] takes no `instructions` — they describe the server, not one \
                     host, so they are declared once: McpModule::for_root(McpOptions { \
                     server: Some(McpIdentity::new(name, version).instructions(\"…\")), \
                     ..Default::default() }). What this host's tools *do* belongs to \
                     each #[tool(description = \"…\")]",
                ));
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[mcp] accepts `path`, `name`, `version` and `title`, all optional",
                ));
            }
        }
    }
    // A version names nothing on its own, and inheriting the app's name while
    // overriding its version would report a server that does not exist.
    if let (None, Some(version)) = (&parsed.name, &parsed.version) {
        return Err(syn::Error::new_spanned(
            version,
            "#[mcp] `version` needs a `name` beside it — without one the endpoint \
             reports the app's name at another version. Drop it to inherit the app's \
             identity whole",
        ));
    }
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
    if path.value().is_empty() {
        return Err(syn::Error::new_spanned(
            path,
            "#[mcp] `path` is empty — drop the argument entirely to serve the \
             default endpoint, which is what a bare #[mcp] means",
        ));
    }
    Ok(())
}

/// An optional identity argument as the `Option<&str>` tokens
/// `McpIdentity::declared` takes.
fn opt_str(expr: Option<&Expr>) -> TokenStream2 {
    match expr {
        Some(value) => quote! { ::core::option::Option::Some(#value) },
        None => quote! { ::core::option::Option::None },
    }
}
