//! Emit `Create<Name>` / `Update<Name>` from `#[expose(input(...))]`
//! fields. `validate(...)` bodies are re-emitted verbatim as `#[validate(...)]`
//! so REST `Valid<Json<…>>` and the service enforce the same rules.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{ResourceField, ResourceModel, graphql_crate_attr, graphql_object_derive};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
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
        let ty = &f.ty;
        let validate = f.validate.iter().map(|body| quote! { #[validate(#body)] });
        quote! {
            #(#validate)*
            pub #name: #ty
        }
    });

    let graphql_derives = graphql_object_derive(model, "InputObject");
    let graphql_crate = graphql_crate_attr(model);

    quote! {
        // Same routing as the wire DTO: these inputs are generated, so their
        // derives must not reach for the entity crate's extern prelude.
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::nest_rs_resource::serde::Deserialize,
            #graphql_derives
            ::nest_rs_resource::validator::Validate,
            ::nest_rs_resource::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_resource::serde")]
        #[validate(crate = ::nest_rs_resource::validator)]
        #[schemars(crate = "::nest_rs_resource::schemars")]
        #graphql_crate
        pub struct #name {
            #(#decls),*
        }
    }
}
