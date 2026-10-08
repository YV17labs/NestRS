use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Expr;

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    from_container_method, from_scope_method, injected_keyed_method, injected_method,
    injected_names_method, injected_optional_method, optional_dependencies_method,
    parse_provider_host,
};

pub(crate) fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
    let scope = match parse_injectable_scope(args.into()) {
        Ok(s) => s,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = match parse_provider_host(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };

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
    let from_scope = match scope {
        InjectableScope::Request | InjectableScope::Transient => from_scope_method(&ctor),
        InjectableScope::Singleton => TokenStream2::new(),
    };
    let injected = injected_method(&dep_keys);
    // Every scope, so the access graph names a lazily-built provider's missing dependency.
    let injected_names = injected_names_method(&dep_names);
    let injected_keyed = injected_keyed_method(&keyed_dep_keys);
    let injected_optional = injected_optional_method(&opt_keys);

    // Every scope: a missing impl could be written by hand and lie about residency.
    let singleton_marker = nest_rs_codegen::provider_residency(
        &name,
        &item.generics,
        matches!(scope, InjectableScope::Singleton),
    );

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
                    builder.provide_transient::<Self, _>(|__scope| {
                        Self::from_scope(__scope)
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
            #injected_optional

            #register_fn
        }

        #singleton_marker
    }
    .into()
}

#[derive(Clone, Copy)]
enum InjectableScope {
    Singleton,
    Request,
    Transient,
}

const INJECTABLE: nest_rs_codegen::Grammar =
    nest_rs_codegen::Grammar::new("injectable", &["scope"]);

const SCOPES: [&str; 3] = ["singleton", "request", "transient"];

fn parse_injectable_scope(args: TokenStream2) -> syn::Result<InjectableScope> {
    if args.is_empty() {
        return Ok(InjectableScope::Singleton);
    }
    let mut scope: Option<InjectableScope> = None;
    INJECTABLE.parse2(args, |arg| {
        let value = arg.expr()?;
        let Expr::Path(path) = &value else {
            return Err(syn::Error::new_spanned(
                &value,
                nest_rs_codegen::unknown_value(
                    "injectable",
                    "scope",
                    &quote!(#value).to_string(),
                    &SCOPES,
                ),
            ));
        };
        let written = nest_rs_codegen::key_as_written(&path.path);
        scope = Some(match written.as_str() {
            "singleton" => InjectableScope::Singleton,
            "request" => InjectableScope::Request,
            "transient" => InjectableScope::Transient,
            other => {
                return Err(syn::Error::new_spanned(
                    &value,
                    nest_rs_codegen::unknown_value("injectable", "scope", other, &SCOPES),
                ));
            }
        });
        Ok(())
    })?;
    Ok(scope.unwrap_or(InjectableScope::Singleton))
}
