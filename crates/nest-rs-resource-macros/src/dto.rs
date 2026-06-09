//! Emit the GraphQL output object plus its `From<&Model>`. A `skip` field is
//! absent; a `Uuid` renders as `String`. Derives `SimpleObject` + `JsonSchema`
//! so one declaration feeds GraphQL and OpenAPI.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{ResourceModel, complexity_attr, is_uuid};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
    let output = &model.output_ident;
    let source = &model.source_ident;
    let mut decls = Vec::new();
    let mut inits = Vec::new();

    for field in model.fields.iter().filter(|f| f.in_output_struct()) {
        let name = &field.ident;
        // `#[expose(complexity = …)]` on a scalar wire field maps straight
        // through to the SimpleObject derive — async-graphql's visitor honors
        // `#[graphql(complexity = …)]` on SimpleObject fields just as it does
        // on `#[ComplexObject]` resolvers, but SimpleObject discards the
        // expression's argument-variables (a closure with no `let arg = …`
        // binding), so only `child_complexity` and pure literals are safe on a
        // scalar; argument-referencing expressions belong on a hand-rolled
        // `#[field_resolver]` instead.
        let complexity = complexity_attr(&field.complexity, None);
        if is_uuid(&field.ty) {
            decls.push(quote! { #complexity pub #name: ::std::string::String });
            inits.push(quote! { #name: ::std::string::ToString::to_string(&model.#name) });
        } else {
            let ty = &field.ty;
            decls.push(quote! { #complexity pub #name: #ty });
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
            ::serde::Deserialize,
            ::nest_rs_graphql::async_graphql::SimpleObject,
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

/// `<Name>Page` for `#[expose(paginate)]`. `new(items, total, &PageArgs)`
/// derives the page-count and has-more flags so the math lives in one place.
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
            ::serde::Deserialize,
            ::nest_rs_graphql::async_graphql::SimpleObject,
            ::schemars::JsonSchema,
        )]
        pub struct #page {
            pub items: ::std::vec::Vec<#output>,
            pub total: u64,
            pub page: u64,
            pub per_page: u64,
            /// `ceil(total / per_page)`.
            pub total_pages: u64,
            pub has_next_page: bool,
            pub has_previous_page: bool,
        }

        impl #page {
            pub fn new(
                items: ::std::vec::Vec<#output>,
                total: u64,
                args: &::nest_rs_resource::PageArgs,
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
