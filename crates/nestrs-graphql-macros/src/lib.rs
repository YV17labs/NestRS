//! GraphQL decorator macros, re-exported by `nestrs-graphql`. The generated
//! code uses absolute paths (`::nestrs_graphql::*`, `::std::sync::Arc`), so this
//! crate does not depend on them — they resolve at the call site.
//!
//! Mirrors the HTTP `#[controller]`/`#[routes]` split: `#[resolver]` on a struct
//! handles construction (DI); `#[resolver]` on its impl block orchestrates the
//! method-level `#[query]`/`#[mutation]` verbs.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, Ident, ImplItem, Item, ItemImpl, ItemStruct, Type};

use nestrs_macro_support::{
    build_injectable_body, forwarded_arg_idents, from_container_method, InjectableBody,
};

/// Mark a GraphQL resolver.
///
/// Applied in two places, like `#[controller]` + `#[routes]`:
///
/// - **On the struct** — builds it from the container (`#[inject]` fields
///   resolved, others default), emitting `from_container`. The resolver is not
///   a provider; it is constructed at schema-build time.
/// - **On its impl block** — each method tagged `#[query]` or `#[mutation]` is
///   split into a generated `#[Object]` root (`__<Name>Query` /
///   `__<Name>Mutation`) that delegates back to the inherent method, and is
///   submitted to the link-time registry. The schema composes itself from that
///   registry (see `nestrs_graphql::build_schema`) — there is no central list.
#[proc_macro_attribute]
pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[resolver] takes no arguments; tag methods with `#[query]` / `#[mutation]`",
        )
        .to_compile_error()
        .into();
    }

    match parse_macro_input!(input as Item) {
        Item::Struct(item) => resolver_struct(item),
        Item::Impl(item) => resolver_impl(item),
        other => syn::Error::new_spanned(
            other,
            "#[resolver] applies to a struct (construction) or its impl block (query/mutation methods)",
        )
        .to_compile_error()
        .into(),
    }
}

/// `#[resolver]` on the struct: construction only, like `#[injectable]`.
fn resolver_struct(mut item: ItemStruct) -> TokenStream {
    let InjectableBody { ctor, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }
    }
    .into()
}

/// `#[resolver]` on the impl: split `#[query]`/`#[mutation]` methods into
/// generated `#[Object]` roots and register them.
fn resolver_impl(mut item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();

    let base = match &*self_ty {
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.clone()),
        _ => None,
    };
    let Some(base) = base else {
        return syn::Error::new_spanned(
            &self_ty,
            "#[resolver] on an impl requires a simple struct path (e.g. `impl UsersResolver`)",
        )
        .to_compile_error()
        .into();
    };

    let query_obj = format_ident!("__{}Query", base);
    let mutation_obj = format_ident!("__{}Mutation", base);

    let mut query_methods: Vec<TokenStream2> = Vec::new();
    let mut mutation_methods: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let kind_idx = method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("query") || a.path().is_ident("mutation"));
        let Some(idx) = kind_idx else { continue };

        let kind_attr = method.attrs.remove(idx);
        let is_query = kind_attr.path().is_ident("query");

        // The delegating `#[Object]` method keeps the verbatim signature (with
        // arg attributes) and any remaining attrs — async-graphql's
        // `#[graphql(...)]` belongs here. The inherent method, kept clean,
        // holds the real body.
        let deleg_attrs = method.attrs.clone();
        let sig = method.sig.clone();
        let method_name = method.sig.ident.clone();
        let arg_idents = match forwarded_arg_idents(&sig) {
            Ok(idents) => idents,
            Err(err) => return err.to_compile_error().into(),
        };

        let call = if sig.asyncness.is_some() {
            quote! { self.0.#method_name(#(#arg_idents),*).await }
        } else {
            quote! { self.0.#method_name(#(#arg_idents),*) }
        };

        let delegating = quote! {
            #(#deleg_attrs)*
            #sig { #call }
        };
        if is_query {
            query_methods.push(delegating);
        } else {
            mutation_methods.push(delegating);
        }

        // Clean the inherent method: keep only doc comments, drop arg attrs.
        method.attrs.retain(|a| a.path().is_ident("doc"));
        for input in method.sig.inputs.iter_mut() {
            if let FnArg::Typed(pt) = input {
                pt.attrs.clear();
            }
        }
    }

    let query_block = root_object(&query_obj, &self_ty, &query_methods, quote!(Query));
    let mutation_block = root_object(&mutation_obj, &self_ty, &mutation_methods, quote!(Mutation));

    quote! {
        #item

        #query_block
        #mutation_block
    }
    .into()
}

/// Emit one generated `#[Object]` root + its registry submission, or nothing
/// when the resolver has no methods of that kind.
fn root_object(
    obj: &Ident,
    self_ty: &Type,
    methods: &[TokenStream2],
    kind: TokenStream2,
) -> TokenStream2 {
    if methods.is_empty() {
        return quote!();
    }
    quote! {
        #[allow(non_camel_case_types)]
        pub struct #obj(::std::sync::Arc<#self_ty>);

        #[::nestrs_graphql::async_graphql::Object]
        impl #obj {
            #(#methods)*
        }

        ::nestrs_graphql::inventory::submit! {
            ::nestrs_graphql::ResolverRegistration {
                kind: ::nestrs_graphql::ResolverKind::#kind,
                type_info: |__r| __r.create_fake_output_type::<#obj>(),
                build: |__c| ::std::boxed::Box::new(
                    #obj(::std::sync::Arc::new(<#self_ty>::from_container(__c)))
                ) as ::std::boxed::Box<dyn ::nestrs_graphql::ResolverObject>,
            }
        }
    }
}
