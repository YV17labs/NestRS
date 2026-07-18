use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{ParseStream, Parser};
use syn::{Ident, ItemStruct, Token, parse_macro_input};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    from_container_method, from_scope_method, injected_keyed_method, injected_method,
    injected_names_method, optional_dependencies_method,
};

pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    let scope = match parse_injectable_scope(args.into()) {
        Ok(s) => s,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
        keyed_dep_keys,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // Only a request-scoped provider needs the scope-aware constructor; emitting
    // it elsewhere would reference `RequestScope` for no reason.
    let from_scope = match scope {
        InjectableScope::Request => from_scope_method(&ctor),
        InjectableScope::Singleton | InjectableScope::Transient => TokenStream2::new(),
    };
    let injected = injected_method(&dep_keys);
    // Emitted for every scope (aligned with `injected`), so the access graph can
    // name a missing dependency of a lazily-built scoped/transient provider.
    let injected_names = injected_names_method(&dep_names);
    let injected_keyed = injected_keyed_method(&keyed_dep_keys);

    // Request-scoped and transient: lazy build, no register-phase ordering deps,
    // each registers a factory not a value. `injected` is still reported for
    // the access graph regardless of build timing.
    let (dependencies, dependency_names, optional_dependencies, register_fn) = match scope {
        InjectableScope::Singleton => (
            dependencies_method(&dep_keys),
            dependency_names_method(&dep_names),
            optional_dependencies_method(&opt_keys),
            quote! {
                fn register(
                    builder: ::nest_rs_core::ContainerBuilder,
                ) -> ::nest_rs_core::ContainerBuilder {
                    let __snapshot = builder.snapshot();
                    let __value = Self::from_container(&__snapshot);
                    builder.provide(__value)
                }
            },
        ),
        InjectableScope::Request => (
            dependencies_method(&[]),
            dependency_names_method(&[]),
            optional_dependencies_method(&[]),
            quote! {
                fn register(
                    builder: ::nest_rs_core::ContainerBuilder,
                ) -> ::nest_rs_core::ContainerBuilder {
                    builder.provide_scoped::<Self, _>(|__scope| {
                        Self::from_scope(__scope)
                    })
                }
            },
        ),
        InjectableScope::Transient => (
            dependencies_method(&[]),
            dependency_names_method(&[]),
            optional_dependencies_method(&[]),
            quote! {
                fn register(
                    builder: ::nest_rs_core::ContainerBuilder,
                ) -> ::nest_rs_core::ContainerBuilder {
                    builder.provide_transient::<Self, _>(|__container| {
                        Self::from_container(__container)
                    })
                }
            },
        ),
    };

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
            #from_scope
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected
            #injected_names
            #injected_keyed

            #register_fn
        }
    }
    .into()
}

#[derive(Clone, Copy)]
enum InjectableScope {
    Singleton,
    Request,
    Transient,
}

/// Parse `#[injectable(scope = singleton|request|transient)]`. Empty defaults
/// to [`InjectableScope::Singleton`].
fn parse_injectable_scope(args: TokenStream2) -> syn::Result<InjectableScope> {
    if args.is_empty() {
        return Ok(InjectableScope::Singleton);
    }
    let parser = |input: ParseStream| -> syn::Result<InjectableScope> {
        let key: Ident = input.parse()?;
        if key != "scope" {
            return Err(syn::Error::new(
                key.span(),
                "expected `scope = singleton`, `scope = request`, or `scope = transient`",
            ));
        }
        input.parse::<Token![=]>()?;
        let value: Ident = input.parse()?;
        match value.to_string().as_str() {
            "singleton" => Ok(InjectableScope::Singleton),
            "request" => Ok(InjectableScope::Request),
            "transient" => Ok(InjectableScope::Transient),
            other => Err(syn::Error::new(
                value.span(),
                format!(
                    "unknown scope `{other}` (expected `singleton`, `request`, or `transient`)"
                ),
            )),
        }
    };
    parser.parse2(args)
}
