//! `#[interceptor]` — mark a struct as a **global** HTTP interceptor (for
//! infrastructure that must wrap everything: a DB-transaction context,
//! tracing). The macro attaches an
//! [`HttpEndpointWrap`](::nest_rs_http::HttpEndpointWrap) but does *not*
//! register the type as a provider — it is mounted automatically. To bind
//! per-controller/handler, write a plain `#[injectable] + impl Interceptor`
//! and list it in `#[use_interceptors(...)]`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{ItemStruct, Meta, Token, parse_macro_input};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    from_container_method, injected_method, optional_dependencies_method,
};

fn parse_priority(args: TokenStream) -> syn::Result<TokenStream2> {
    if args.is_empty() {
        return Ok(quote! { ::nest_rs_http::endpoint_wrap_priority::INTERCEPTORS });
    }
    // **The whole list, then exactly one of it.** `syn::parse::<Meta>` consumes
    // one and reports the rest as syn's "unexpected token", which names neither
    // the key nor the fact — so `#[interceptor(priority = 1, priority = 2)]`
    // died on the grammar rather than on the duplicate, and the shared sentence
    // this decorator already uses for an unknown key had no counterpart for a
    // repeated one.
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(TokenStream2::from(args))?;
    let mut metas = metas.into_iter();
    let meta = metas.next().expect("a non-empty argument list");
    if let Some(second) = metas.next() {
        return Err(syn::Error::new_spanned(
            second,
            nest_rs_codegen::duplicate_argument("interceptor", "priority"),
        ));
    }
    // Both questions through the shared helper that answers them together: a
    // bare `#[interceptor(priority)]` is a `Meta::Path`, and so is a bare
    // *unknown* key — one wording said "expected `priority = <integer>`" to
    // both, which is right for the first and false for the second.
    let Meta::NameValue(nv) = meta else {
        return Err(nest_rs_codegen::unmatched_meta(
            "interceptor",
            &meta,
            &["priority"],
        ));
    };
    if !nv.path.is_ident("priority") {
        let name = nest_rs_codegen::key_as_written(&nv.path);
        return Err(syn::Error::new(
            nv.path.span(),
            nest_rs_codegen::unknown_argument("interceptor", &name, &["priority"]),
        ));
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Int(lit),
        ..
    }) = nv.value
    else {
        return Err(syn::Error::new(
            nv.value.span(),
            "priority must be an integer",
        ));
    };
    let priority: i32 = lit.base10_parse()?;
    Ok(quote! { #priority })
}

pub(crate) fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    let priority = match parse_priority(args) {
        Ok(p) => p,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        opt_keys,
        ..
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let dependencies = dependencies_method(&dep_keys);
    let dependency_names = dependency_names_method(&dep_names);
    let optional_dependencies = optional_dependencies_method(&opt_keys);
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            #dependencies
            #dependency_names
            #optional_dependencies
            #injected

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                let __snapshot = builder.snapshot();
                let __value = Self::from_container(&__snapshot);
                let __arc: ::std::sync::Arc<dyn ::nest_rs_interceptors::Interceptor> =
                    ::std::sync::Arc::new(__value);
                builder.attach_meta::<Self, ::nest_rs_http::HttpEndpointWrap>(
                    ::nest_rs_http::HttpEndpointWrap::with_priority(
                        #priority,
                        move |_container, __endpoint| {
                        ::nest_rs_http::poem::EndpointExt::boxed(::nest_rs_http::poem::EndpointExt::map_to_response(
                            ::nest_rs_interceptors::InterceptorExt::interceptor(
                                __endpoint,
                                ::std::sync::Arc::clone(&__arc),
                            ),
                        ))
                    },
                    ),
                )
            }
        }
    }
    .into()
}
