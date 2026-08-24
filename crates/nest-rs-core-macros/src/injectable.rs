use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, Meta, Token};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    from_container_method, from_scope_method, injected_keyed_method, injected_method,
    injected_names_method, optional_dependencies_method, parse_provider_host,
};

pub fn injectable(args: TokenStream, input: TokenStream) -> TokenStream {
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
    // Request-scoped and transient providers both resolve their `#[inject]` deps
    // through a `RequestScope` (so a request-scoped dep is shared with the rest
    // of the request), so both need the scope-aware constructor. A singleton
    // never sees a scope, so emitting it there would reference `RequestScope`
    // for no reason.
    let from_scope = match scope {
        InjectableScope::Request | InjectableScope::Transient => from_scope_method(&ctor),
        InjectableScope::Singleton => TokenStream2::new(),
    };
    let injected = injected_method(&dep_keys);
    // Emitted for every scope (aligned with `injected`), so the access graph can
    // name a missing dependency of a lazily-built scoped/transient provider.
    let injected_names = injected_names_method(&dep_names);
    let injected_keyed = injected_keyed_method(&keyed_dep_keys);

    // What the container will hold under this type, stated for **every** scope:
    // a missing impl is fillable by hand, and omitting it is how a
    // `scope = transient` host once slipped through the bound that refuses it.
    let singleton_marker = nest_rs_codegen::provider_residency(
        &name,
        &item.generics,
        matches!(scope, InjectableScope::Singleton),
    );

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

/// Parse `#[injectable(scope = singleton|request|transient)]`. Empty defaults
/// to [`InjectableScope::Singleton`].
///
/// **Three cases used to reach syn rather than a sentence**, on the most-written
/// decorator in either workspace — the struct half of all five `on_provider`
/// pairs. `#[injectable(scope)]` died on `` expected `=` ``, which
/// [`nest_rs_codegen::needs_a_value`] exists to replace ("a bare `expected `=``
/// names the grammar and not the key"); and a duplicate or trailing argument
/// died on `Parser::parse2`'s full-consumption requirement with "unexpected
/// token", naming neither. Only the unknown-*value* case had a refusal, and it
/// is the one that had a snapshot.
fn parse_injectable_scope(args: TokenStream2) -> syn::Result<InjectableScope> {
    if args.is_empty() {
        return Ok(InjectableScope::Singleton);
    }
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut scope: Option<InjectableScope> = None;
    for meta in &metas {
        let Meta::NameValue(nv) = meta else {
            return Err(nest_rs_codegen::unmatched_meta(
                "injectable",
                meta,
                &["scope"],
            ));
        };
        if !nv.path.is_ident("scope") {
            return Err(nest_rs_codegen::unmatched_meta(
                "injectable",
                meta,
                &["scope"],
            ));
        }
        nest_rs_codegen::reject_duplicate_argument(scope.is_some(), meta, "injectable", "scope")?;
        let value_text = quote!(#nv).to_string();
        let Expr::Path(path) = &nv.value else {
            return Err(syn::Error::new_spanned(
                &nv.value,
                nest_rs_codegen::unknown_value(
                    "injectable",
                    "scope",
                    &value_text,
                    &["singleton", "request", "transient"],
                ),
            ));
        };
        let value = nest_rs_codegen::key_as_written(&path.path);
        scope = Some(match value.as_str() {
            "singleton" => InjectableScope::Singleton,
            "request" => InjectableScope::Request,
            "transient" => InjectableScope::Transient,
            other => {
                return Err(syn::Error::new_spanned(
                    &nv.value,
                    nest_rs_codegen::unknown_value(
                        "injectable",
                        "scope",
                        other,
                        &["singleton", "request", "transient"],
                    ),
                ));
            }
        });
    }
    Ok(scope.unwrap_or(InjectableScope::Singleton))
}
