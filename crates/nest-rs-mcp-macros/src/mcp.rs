use nest_rs_codegen::__private::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Expr, ItemStruct, LitStr};

use nest_rs_codegen::__private::layers::{LayerAttribute, LayerScope, Layers, take_layers};
use nest_rs_codegen::__private::{
    InjectableBody, MCP_GRAMMAR, MountPath, VersionedEdge, build_injectable_body,
    from_container_method, guard_capability_bounds, host_meta_fn, injected_keys_with_layers,
    injected_names_with_layers, injected_optional_method, injected_optional_names_method,
    layer_deps, reject_path, scoped_specs,
};

pub(crate) fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    match pair::MCP.parse_host(input.into()) {
        Ok(item) => mcp_struct(args, item),
        Err(err) => err.to_compile_error().into(),
    }
}

pub(crate) fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = match pair::MCP.parse_operations(input.into()) {
        Ok(item) => crate::mcp_impl::mcp_impl(args, item),
        Err(err) => err.to_compile_error().into(),
    };
    // The struct half bounds the host on `McpHost`, so a refused block still emits
    // a default `ServerHandler` or the refusal gains an unsatisfied-bound error.
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
    if !item.generics.params.is_empty() {
        let err = syn::Error::new_spanned(
            &item.generics,
            "`#[mcp]` must be on a concrete struct — each operation `#[tools]` declares on it \
             runs a `static` site, which no parameter of the struct reaches, so a generic \
             host's instances would share one site; write a non-generic host type per \
             instantiation",
        );
        return refused_beside(err, item);
    }

    let Layers {
        guards,
        interceptors,
        exception_filters,
        meta,
        ..
    } = match take_layers(&pair::MCP, LayerScope::Host, &mut item.attrs) {
        Ok(layers) => layers,
        Err(err) => return refused_beside(err, item),
    };
    let host_meta = host_meta_fn(&meta);
    let capability_bounds = guard_capability_bounds(
        guards.iter(),
        quote!(::nest_rs_guards::GuardFor<::nest_rs_mcp::McpView>),
    );

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
        opt_names,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return refused_beside(err, item),
    };

    let name = item.ident.clone();
    let host_name = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // Per-operation layers arrive from the impl half through
    // `__nestrs_mcp_operation_layers()`: a hand-written `ServerHandler` host has none.
    let layers = layer_deps(guards.iter().chain(&interceptors).chain(&exception_filters));
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);
    let injected_optional = injected_optional_method(&opt_keys);
    let injected_optional_names = injected_optional_names_method(&opt_names);
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));
    let interceptor_specs = quote! {
        ::std::vec![#(::nest_rs_core::interceptor::<#interceptors>()),*]
    };
    let exception_filter_specs = quote! {
        ::std::vec![#(::nest_rs_core::exception_filter::<#exception_filters>()),*]
    };

    // Empty means "none declared": `nest-rs-mcp` owns the default path.
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

            #host_meta

            /// Host-scope `#[use_guards(...)]`, read by the impl half when it
            /// composes an operation's guard stage.
            #[doc(hidden)]
            pub fn __nestrs_mcp_host_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::GuardSpec>
            {
                #guard_specs
            }

            /// Host-scope `#[use_interceptors(...)]`, read by the impl half when
            /// it declares an operation's site.
            #[doc(hidden)]
            pub fn __nestrs_mcp_host_interceptor_specs()
                -> ::std::vec::Vec<::nest_rs_core::InterceptorSpec>
            {
                #interceptor_specs
            }

            /// Host-scope `#[use_exception_filters(...)]`, read by the impl half
            /// when it declares an operation's site.
            #[doc(hidden)]
            pub fn __nestrs_mcp_host_exception_filter_specs()
                -> ::std::vec::Vec<::nest_rs_core::ExceptionFilterSpec>
            {
                #exception_filter_specs
            }
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
                // An inherent associated fn shadows the trait fallback, which
                // answers empty for a host that writes rmcp by hand.
                use ::nest_rs_mcp::__private::DefaultOperationLayers as _;
                let mut __keys: ::std::vec::Vec<::core::any::TypeId> = #injected_keys;
                __keys.extend(<Self>::__nestrs_mcp_operation_layers().0);
                __keys
            }

            fn injected_names() -> ::std::vec::Vec<&'static str> {
                use ::nest_rs_mcp::__private::DefaultOperationLayers as _;
                let mut __names: ::std::vec::Vec<&'static str> = #injected_names;
                __names.extend(<Self>::__nestrs_mcp_operation_layers().1);
                __names
            }

            #injected_optional

            #injected_optional_names

            fn injected_per_request() -> ::std::vec::Vec<(::core::any::TypeId, &'static str)> {
                // The impl half's probe of each operation's parameters; empty for a
                // host that writes rmcp by hand.
                use ::nest_rs_mcp::__private::DefaultOperationLayers as _;
                <Self>::__nestrs_mcp_injected_per_request()
            }

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                static __NESTRS_MCP_ENDPOINT: ::nest_rs_core::Handler =
                    ::nest_rs_mcp::__private::declare_endpoint(#path);
                ::nest_rs_mcp::__private::register_host::<Self>(
                    builder,
                    &__NESTRS_MCP_ENDPOINT,
                    #path,
                    #host_name,
                    ::nest_rs_mcp::__private::declared_identity(#identity_name, #identity_title),
                    |__c| -> ::std::sync::Arc<dyn ::nest_rs_mcp::McpHost> {
                        ::std::sync::Arc::new(<Self>::__nestrs_from_container(__c))
                    },
                    || {
                        // An inherent router (the impl half's `pub(crate)` one, or
                        // rmcp's) shadows each empty fallback; the mount routes a
                        // name, and the boot checks it is claimed once, on these.
                        use ::nest_rs_mcp::__private::{
                            DefaultDeclaredRoutes as _, DefaultPromptRouter as _,
                            DefaultToolRouter as _,
                        };
                        ::nest_rs_mcp::__private::DeclaredRoutes::new(
                            <Self>::tool_router().list_all(),
                            <Self>::prompt_router().list_all(),
                            <Self>::__nestrs_mcp_declares_every_route(),
                        )
                    },
                    || {
                        use ::nest_rs_mcp::__private::DefaultOperationLayers as _;
                        <Self>::__nestrs_mcp_operation_sites()
                    },
                )
            }
        }
    }
    .into()
}

/// A refused `#[mcp(..)]` argument, reported beside the struct as written so the
/// `#[tools]` impl does not cascade into `cannot find type`; the consumed helper
/// attributes come off, or rustc reports them as unknown.
fn refused_beside(err: syn::Error, mut item: ItemStruct) -> TokenStream {
    item.attrs.retain(|attr| {
        !LayerAttribute::ALL
            .iter()
            .any(|consumed| attr.path().is_ident(consumed.name()))
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
                -> ::std::vec::Vec<::nest_rs_guards::GuardSpec>
            {
                ::std::vec::Vec::new()
            }

            #[doc(hidden)]
            pub fn __nestrs_mcp_host_interceptor_specs()
                -> ::std::vec::Vec<::nest_rs_core::InterceptorSpec>
            {
                ::std::vec::Vec::new()
            }

            #[doc(hidden)]
            pub fn __nestrs_mcp_host_exception_filter_specs()
                -> ::std::vec::Vec<::nest_rs_core::ExceptionFilterSpec>
            {
                ::std::vec::Vec::new()
            }
        }
    }
    .into()
}

/// Everything `#[mcp(..)]` accepts. The identity arguments stay expressions
/// (`name = env!("CARGO_PKG_NAME")`); `declared_identity`'s `Option<&str>`
/// parameters type-check them.
#[derive(Default)]
struct McpArgs {
    path: Option<LitStr>,
    name: Option<Expr>,
    title: Option<Expr>,
}

fn parse_mcp_args(args: TokenStream2) -> syn::Result<McpArgs> {
    // Before the grammar, so `version` gets its own refusal, not the unknown-key one.
    VersionedEdge::Mcp.reject_version(&args)?;
    let mut parsed = McpArgs::default();
    MCP_GRAMMAR.parse2(args, |arg| {
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

/// Refuses an empty `path`: absent is the one spelling of "the default".
fn check_path(path: &LitStr) -> syn::Result<()> {
    reject_path("mcp", path, MountPath::Literal)
}

/// An optional identity argument as the `Option<&str>` tokens
/// `declared_identity` takes.
fn opt_str(expr: Option<&Expr>) -> TokenStream2 {
    match expr {
        Some(value) => quote! { ::core::option::Option::Some(#value) },
        None => quote! { ::core::option::Option::None },
    }
}
