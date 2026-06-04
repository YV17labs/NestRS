use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{ParseStream, Parser};
use syn::{Ident, ItemStruct, Token, parse_macro_input};

use nestrs_codegen::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    from_container_method, injected_method, optional_dependencies_method,
};

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

    // Request-scoped: lazy build, no register-phase ordering deps, registers
    // a factory not a value. `injected` is still reported for the access graph.
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

/// Parse `#[injectable(scope = request|singleton)]`. Returns `true` for
/// request-scoped; empty or `singleton` returns `false`.
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
