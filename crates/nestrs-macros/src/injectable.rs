//! `#[injectable]`: mark a struct as a container-constructed provider. See the
//! entry doc in `lib.rs`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{ParseStream, Parser};
use syn::{parse_macro_input, Ident, ItemStruct, Token};

use nestrs_codegen::{
    build_injectable_body, dependencies_method, dependency_names_method, from_container_method,
    injected_method, optional_dependencies_method, InjectableBody,
};

/// `#[injectable]` entry: applies to a provider struct.
pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    let request_scoped = match parse_injectable_scope(args.into()) {
        Ok(scoped) => scoped,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);

    // A request-scoped provider builds lazily (per request), so — exactly like a
    // controller — it declares no register-phase `dependencies`/ordering and
    // registers a factory rather than a singleton value. `injected` is reported
    // regardless so the access-graph still governs its `#[inject]` keys.
    let (dependencies, dependency_names, optional_dependencies, register_fn) = if request_scoped {
        (
            dependencies_method(&[]),
            dependency_names_method(&[]),
            optional_dependencies_method(&[]),
            quote! {
                fn register(
                    builder: ::nestrs_core::ContainerBuilder,
                ) -> ::nestrs_core::ContainerBuilder {
                    builder.provide_scoped::<Self, _>(|__container| {
                        Self::from_container(__container)
                    })
                }
            },
        )
    } else {
        (
            dependencies_method(&dep_keys),
            dependency_names_method(&dep_names),
            optional_dependencies_method(&opt_keys),
            quote! {
                fn register(
                    builder: ::nestrs_core::ContainerBuilder,
                ) -> ::nestrs_core::ContainerBuilder {
                    let __snapshot = builder.snapshot();
                    let __value = Self::from_container(&__snapshot);
                    builder.provide(__value)
                }
            },
        )
    };

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected

            #register_fn
        }
    }
    .into()
}

/// Parse the optional `#[injectable(scope = …)]` argument. Empty (or
/// `scope = singleton`) is the default singleton provider; `scope = request`
/// marks the provider request-scoped. Returns `true` when request-scoped.
fn parse_injectable_scope(args: TokenStream2) -> syn::Result<bool> {
    if args.is_empty() {
        return Ok(false);
    }
    let parser = |input: ParseStream| -> syn::Result<bool> {
        let key: Ident = input.parse()?;
        if key != "scope" {
            return Err(syn::Error::new(
                key.span(),
                "expected `scope = request` or `scope = singleton`",
            ));
        }
        input.parse::<Token![=]>()?;
        let value: Ident = input.parse()?;
        match value.to_string().as_str() {
            "request" => Ok(true),
            "singleton" => Ok(false),
            other => Err(syn::Error::new(
                value.span(),
                format!("unknown scope `{other}` (expected `request` or `singleton`)"),
            )),
        }
    };
    parser.parse2(args)
}
