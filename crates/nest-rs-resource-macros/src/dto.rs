//! Emit the wire output object plus its `From<&Model>`. Only `#[expose]`d
//! columns appear — an unexposed field is absent; a `Uuid` renders as `String`
//! on the wire. Derives `JsonSchema` for OpenAPI; with `graphql`, also
//! `SimpleObject`.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{ResourceModel, complexity_attr, graphql_object_derive, is_datetime_tz, is_uuid};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
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
        if is_uuid(&field.ty) {
            decls.push(quote! { #complexity pub #name: ::std::string::String });
            inits.push(quote! { #name: ::std::string::ToString::to_string(&model.#name) });
        } else if is_datetime_tz(&field.ty) {
            decls.push(quote! { #complexity pub #name: ::std::string::String });
            inits.push(quote! {
                #name: ::nest_rs_resource::chrono::DateTime::<::nest_rs_resource::chrono::FixedOffset>::to_rfc3339(&model.#name)
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

    quote! {
        // Derives routed through the surface crate, each with its `crate = `
        // override: a derive expands against the *call site's* prelude, so
        // without the override the generated model would oblige the entity
        // crate to declare `serde` and `schemars` for code it never wrote.
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::nest_rs_resource::serde::Serialize,
            ::nest_rs_resource::serde::Deserialize,
            #graphql_derives
            ::nest_rs_resource::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_resource::serde")]
        #[schemars(crate = "::nest_rs_resource::schemars")]
        #complex
        pub struct #output {
            #(#decls),*
        }

        impl ::core::convert::From<&#source> for #output {
            fn from(model: &#source) -> Self {
                Self { #(#inits),* }
            }
        }
    }
}
