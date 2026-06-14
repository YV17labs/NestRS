//! Emit `Create<Name>Dto` / `Update<Name>Dto` from `#[expose(input(...))]`
//! fields. `validate(...)` bodies are re-emitted verbatim as `#[validate(...)]`
//! so REST `Valid<Json<…>>` and the service enforce the same rules.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{ResourceField, ResourceModel, graphql_object_derive};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
    let create = input_struct(&model.create_dto_ident, model, |f| f.in_create);
    let update = input_struct(&model.update_dto_ident, model, |f| f.in_update);
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
        let ty = &f.ty;
        let validate = f.validate.iter().map(|body| quote! { #[validate(#body)] });
        quote! {
            #(#validate)*
            pub #name: #ty
        }
    });

    let graphql_derives = graphql_object_derive(model, "InputObject");

    quote! {
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::serde::Deserialize,
            #graphql_derives
            ::validator::Validate,
            ::schemars::JsonSchema,
        )]
        pub struct #name {
            #(#decls),*
        }
    }
}
