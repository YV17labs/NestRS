//! Emit `CreateModel` / `UpdateModel` impls — the conversions `CrudService`'s
//! default `create`/`update` call. Only `input(create)` / `input(update)`
//! columns are set; server-side columns (`org_id`, etc.) stay `NotSet`, so an
//! entity needing them overrides the service method.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{ResourceModel, is_uuid};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
    let create = emit_create(model);
    let update = emit_update(model);
    quote! {
        #create
        #update
    }
}

fn emit_create(model: &ResourceModel) -> TokenStream2 {
    let setters: Vec<TokenStream2> = model
        .fields
        .iter()
        .filter(|f| f.in_create && !f.is_pk)
        .map(|f| {
            let name = &f.ident;
            quote! { __am.#name = ::sea_orm::ActiveValue::Set(self.#name); }
        })
        .collect();
    if setters.is_empty() {
        return quote! {};
    }

    // A non-`Uuid` PK (e.g. auto-increment) is left `NotSet` for the DB.
    let pk_seed = match model.fields.iter().find(|f| f.is_pk) {
        Some(pk) if is_uuid(&pk.ty) => {
            let id = &pk.ident;
            quote! { __am.#id = ::sea_orm::ActiveValue::Set(::uuid::Uuid::now_v7()); }
        }
        _ => quote! {},
    };

    let create = &model.create_ident;
    quote! {
        impl ::nest_rs_seaorm::CreateModel<Entity> for #create {
            fn into_active_model(self) -> ActiveModel {
                let mut __am = <ActiveModel as ::core::default::Default>::default();
                #pk_seed
                #(#setters)*
                __am
            }
        }
    }
}

fn emit_update(model: &ResourceModel) -> TokenStream2 {
    let setters: Vec<TokenStream2> = model
        .fields
        .iter()
        .filter(|f| f.in_update)
        .map(|f| {
            let name = &f.ident;
            quote! { __am.#name = ::sea_orm::ActiveValue::Set(self.#name); }
        })
        .collect();
    if setters.is_empty() {
        return quote! {};
    }

    let update = &model.update_ident;
    quote! {
        impl ::nest_rs_seaorm::UpdateModel<Entity> for #update {
            fn apply_to(self, mut __am: ActiveModel) -> ActiveModel {
                #(#setters)*
                __am
            }
        }
    }
}
