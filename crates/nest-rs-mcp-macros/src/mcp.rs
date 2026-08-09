use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, Item, ItemStruct, LitStr, Meta, Token};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, expr_str, from_container_method, injected_method,
};

pub(crate) fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    // One decorator, two item shapes: the struct is the host, the impl is its
    // operations. Same name on both because they are one concern — and because
    // a `#[tools]` sitting one letter from the `#[tool]` beneath it would read
    // as a typo at every glance.
    match syn::parse::<Item>(input) {
        Ok(Item::Impl(item)) => crate::mcp_impl::mcp_impl(args, item),
        Ok(Item::Struct(item)) => mcp_struct(args, item),
        // Parsing once and naming both shapes keeps a broken `impl` from being
        // reported as "expected struct" — the confusing-error-inside-a-macro
        // failure every diagnostic in this crate works to avoid.
        Ok(other) => syn::Error::new_spanned(
            other,
            "#[mcp] decorates a host struct or the inherent impl holding its \
             #[tool] / #[prompt] methods",
        )
        .to_compile_error()
        .into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn mcp_struct(args: TokenStream, mut item: ItemStruct) -> TokenStream {
    let args = match parse_mcp_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let host_name = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);

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

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

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
