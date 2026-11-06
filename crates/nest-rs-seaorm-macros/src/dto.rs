//! Emit the wire output object, its `From<&Model>` and the entity's `Exposed`
//! impl naming it — the one association from an entity to its wire type,
//! which `#[crud]` and the relation resolvers read. Only `#[expose]`d
//! columns appear — an unexposed field is absent; a `Uuid` or a timestamp
//! renders as a `String` on the wire, its schema keeping the `uuid` or
//! `date-time` format. Derives `JsonSchema` for OpenAPI; with `graphql`, also
//! `SimpleObject`, and says which entity's mask shapes its rows where no posture
//! names one.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{
    ResourceModel, complexity_attr, graphql_crate_attr, graphql_object_derive, graphql_root,
    is_datetime_tz, is_uuid,
};

pub(crate) fn emit(model: &ResourceModel) -> TokenStream2 {
    let output = &model.output_ident;
    let source = &model.source_ident;
    let mut decls = Vec::new();
    let mut inits = Vec::new();

    for field in model.fields.iter().filter(|f| f.in_output_struct()) {
        let name = &field.ident;
        let complexity = if model.graphql {
            complexity_attr(&field.complexity, None)
        } else {
            TokenStream2::new()
        };
        // The `String` keeps the format its column had, or a generated client
        // types an id or a timestamp as free text.
        if is_uuid(&field.ty) {
            decls.push(quote! {
                #complexity
                #[schemars(extend("format" = "uuid"))]
                pub #name: ::std::string::String
            });
            inits.push(quote! { #name: ::std::string::ToString::to_string(&model.#name) });
        } else if is_datetime_tz(&field.ty) {
            decls.push(quote! {
                #complexity
                #[schemars(extend("format" = "date-time"))]
                pub #name: ::std::string::String
            });
            // chrono's own `Serialize` spelling (`Z` for UTC), which the round
            // trip a mask runs writes too: one instant, one string.
            inits.push(quote! {
                #name: ::nest_rs_seaorm::__private::chrono::DateTime::<::nest_rs_seaorm::__private::chrono::FixedOffset>::to_rfc3339_opts(
                    &model.#name,
                    ::nest_rs_seaorm::__private::chrono::SecondsFormat::AutoSi,
                    true,
                )
            });
        } else {
            let ty = &field.ty;
            decls.push(quote! { #complexity pub #name: #ty });
            inits.push(quote! { #name: ::core::clone::Clone::clone(&model.#name) });
        }
    }

    let complex = if model.graphql && model.complex {
        quote! { #[graphql(complex)] }
    } else {
        quote! {}
    };

    let graphql_derives = graphql_object_derive(model, "SimpleObject");
    let graphql_crate = graphql_crate_attr(model);
    let wire_row = model.graphql.then(|| wire_row_impl(output));

    quote! {
        // A derive expands against the call site's prelude: without `crate = `
        // the entity crate would have to declare `serde` and `schemars`.
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::nest_rs_seaorm::__private::serde::Serialize,
            ::nest_rs_seaorm::__private::serde::Deserialize,
            #graphql_derives
            ::nest_rs_seaorm::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_seaorm::__private::serde")]
        #[schemars(crate = "::nest_rs_seaorm::__private::schemars")]
        #graphql_crate
        #complex
        pub struct #output {
            #(#decls),*
        }

        impl ::core::convert::From<&#source> for #output {
            fn from(model: &#source) -> Self {
                Self { #(#inits),* }
            }
        }

        impl ::nest_rs_seaorm::__private::ExposeDeclared for Entity {}

        impl ::nest_rs_seaorm::Exposed for Entity {
            type Wire = #output;
        }

        #wire_row
    }
}

/// What a `#[field_resolver]` answering `output` reads off it: its entity's mask,
/// as no posture names that entity there.
fn wire_row_impl(output: &syn::Ident) -> TokenStream2 {
    let root = graphql_root();
    quote! {
        ::nest_rs_seaorm::__private::inventory::submit! {
            ::nest_rs_seaorm::__private::GraphqlWireType {
                rust_type: ::core::any::type_name::<#output>,
            }
        }

        impl ::nest_rs_seaorm::__private::WireAnswer for #output {
            fn masked(
                self,
                __ctx: &#root::Context<'_>,
            ) -> ::core::result::Result<Self, #root::Error> {
                ::nest_rs_authz::__private::graphql_masked_answer::<Entity, Self>(__ctx, self)
            }

            fn masked_option(
                __answer: ::core::option::Option<Self>,
                __ctx: &#root::Context<'_>,
            ) -> ::core::result::Result<::core::option::Option<Self>, #root::Error> {
                ::nest_rs_authz::__private::graphql_masked_answer::<
                    Entity,
                    ::core::option::Option<Self>,
                >(__ctx, __answer)
            }

            fn masked_list(
                __answer: ::std::vec::Vec<Self>,
                __ctx: &#root::Context<'_>,
            ) -> ::core::result::Result<::std::vec::Vec<Self>, #root::Error> {
                ::nest_rs_authz::__private::graphql_masked_answer::<
                    Entity,
                    ::std::vec::Vec<Self>,
                >(__ctx, __answer)
            }
        }

        impl ::nest_rs_seaorm::__private::WireRow for #output {
            fn masked_nodes(
                __ctx: &#root::Context<'_>,
                __nodes: ::std::vec::Vec<&mut Self>,
            ) -> ::core::result::Result<::std::vec::Vec<bool>, #root::Error> {
                ::nest_rs_authz::__private::graphql_masked_answer_nodes::<Entity, Self>(
                    __ctx, __nodes,
                )
            }
        }
    }
}
