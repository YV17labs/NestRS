//! Emit the GraphQL output object for the entity — the `name` given to
//! `#[expose]` — plus its `From<&Model>`. A `skip` field is absent; a `Uuid`
//! renders as `String`. The type derives `SimpleObject` (GraphQL) and
//! `JsonSchema` (OpenAPI), so one declaration on the entity feeds both surfaces.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{is_uuid, ResourceModel};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
    let output = &model.output_ident;
    let source = &model.source_ident;
    let mut decls = Vec::new();
    let mut inits = Vec::new();

    for field in model.fields.iter().filter(|f| f.in_output()) {
        let name = &field.ident;
        if is_uuid(&field.ty) {
            decls.push(quote! { pub #name: ::std::string::String });
            inits.push(quote! { #name: ::std::string::ToString::to_string(&model.#name) });
        } else {
            let ty = &field.ty;
            decls.push(quote! { pub #name: #ty });
            inits.push(quote! { #name: ::core::clone::Clone::clone(&model.#name) });
        }
    }

    let complex = if model.complex {
        quote! { #[graphql(complex)] }
    } else {
        quote! {}
    };

    let page = emit_page(model);

    quote! {
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::serde::Serialize,
            ::nestrs_graphql::async_graphql::SimpleObject,
            ::schemars::JsonSchema,
        )]
        #complex
        pub struct #output {
            #(#decls),*
        }

        impl ::core::convert::From<&#source> for #output {
            fn from(model: &#source) -> Self {
                Self { #(#inits),* }
            }
        }

        #page
    }
}

/// The `<Name>Page` pagination envelope, emitted only for `#[expose(paginate)]`.
/// A `SimpleObject` + `JsonSchema` (so it serves GraphQL and OpenAPI like the
/// output type), with a `new(items, total, &PageArgs)` that derives the
/// page-count and has-more flags — the math lives here, not at each call site.
fn emit_page(model: &ResourceModel) -> TokenStream2 {
    if !model.paginate {
        return quote! {};
    }
    let output = &model.output_ident;
    let page = &model.page_ident;
    quote! {
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::serde::Serialize,
            ::nestrs_graphql::async_graphql::SimpleObject,
            ::schemars::JsonSchema,
        )]
        pub struct #page {
            /// The rows on this page.
            pub items: ::std::vec::Vec<#output>,
            /// Total rows across all pages.
            pub total: u64,
            /// 1-based page number this envelope represents.
            pub page: u64,
            /// The page size that produced it.
            pub per_page: u64,
            /// `ceil(total / per_page)`.
            pub total_pages: u64,
            /// Whether a page after this one exists.
            pub has_next_page: bool,
            /// Whether a page before this one exists.
            pub has_previous_page: bool,
        }

        impl #page {
            pub fn new(
                items: ::std::vec::Vec<#output>,
                total: u64,
                args: &::nestrs_resource::PageArgs,
            ) -> Self {
                let per_page = ::core::cmp::max(args.per_page, 1);
                let total_pages = total.div_ceil(per_page);
                Self {
                    items,
                    total,
                    page: args.page,
                    per_page: args.per_page,
                    total_pages,
                    has_next_page: args.page < total_pages,
                    has_previous_page: args.page > 1,
                }
            }
        }
    }
}
