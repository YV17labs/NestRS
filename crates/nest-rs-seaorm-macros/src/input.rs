//! Emit `Create<Name>` / `Update<Name>` from `#[expose(input(...))]`
//! fields. `validate(...)` bodies are re-emitted verbatim as `#[validate(...)]`,
//! so every edge decoding one, and a service validating one it built, enforce
//! the same rules. An id is carried as its wire row reads it, `WireId`,
//! GraphQL's `ID`, held to a UUID v7 as a `#[crud]` id argument is.

use nest_rs_codegen::__private::nth_generic_type;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{
    ResourceField, ResourceModel, graphql_crate_attr, graphql_object_derive, is_uuid,
};

/// Whether `field` holds an id, bare or in an `Option`: an input carries it as
/// `WireId`, which the setters turn back into the column's `Uuid`.
pub(crate) fn carries_an_id(field: &ResourceField) -> bool {
    is_uuid(nth_generic_type(&field.ty, "Option", 0).unwrap_or(&field.ty))
}

pub(crate) fn emit(model: &ResourceModel) -> TokenStream2 {
    let create = input_struct(&model.create_ident, model, |f| f.in_create);
    let update = input_struct(&model.update_ident, model, |f| f.in_update);
    quote! {
        #create
        #update
    }
}

fn input_struct(
    name: &syn::Ident,
    model: &ResourceModel,
    include: impl Fn(&ResourceField) -> bool,
) -> TokenStream2 {
    let fields: Vec<_> = model.fields.iter().filter(|f| include(f)).collect();
    if fields.is_empty() {
        return quote! {};
    }

    let decls = fields.iter().map(|f| {
        let name = &f.ident;
        let validate = f.validate.iter().map(|body| quote! { #[validate(#body)] });
        if !carries_an_id(f) {
            let ty = &f.ty;
            return quote! {
                #(#validate)*
                pub #name: #ty
            };
        }
        let id = quote! { ::nest_rs_seaorm::__private::WireId };
        let ty = if nth_generic_type(&f.ty, "Option", 0).is_some() {
            quote! { ::core::option::Option<#id> }
        } else {
            id
        };
        quote! {
            #(#validate)*
            #[validate(custom(function = ::nest_rs_seaorm::__private::uuid_v7))]
            pub #name: #ty
        }
    });

    let graphql_derives = graphql_object_derive(model, "InputObject");
    let graphql_crate = graphql_crate_attr(model);

    quote! {
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::nest_rs_seaorm::__private::serde::Deserialize,
            #graphql_derives
            ::nest_rs_seaorm::__private::validator::Validate,
            ::nest_rs_seaorm::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_seaorm::__private::serde")]
        #[validate(crate = ::nest_rs_seaorm::__private::validator)]
        #[schemars(crate = "::nest_rs_seaorm::__private::schemars")]
        #graphql_crate
        pub struct #name {
            #(#decls),*
        }
    }
}
