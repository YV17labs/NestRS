//! Emit the input → `ActiveModel` conversions `CrudService`'s default `create`
//! and `update` call: `impl CreateModel<Entity> for Create<Name>Input` (a fresh
//! row, its UUID-v7 id seeded) and `impl UpdateModel<Entity> for Update<Name>Input`
//! (the loaded row, its updatable columns overwritten). They are the
//! `nestrs-database` traits, so the generic `CrudService` defaults stay entity-agnostic.
//!
//! Only the columns the developer marked `input(create)` / `input(update)` are
//! set; every other column — a server-side scope column like `org_id`, say —
//! stays `NotSet`. So an entity whose insert needs such a column overrides
//! `create`/`update` on its service (the trait method) and supplies the value.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{is_uuid, ResourceModel};

pub fn emit(model: &ResourceModel) -> TokenStream2 {
    let create = emit_create(model);
    let update = emit_update(model);
    quote! {
        #create
        #update
    }
}

/// `Create<Name>Input::into_active_model` — set the create columns and seed a
/// UUID-v7 primary key. Emitted only when there is at least one create column
/// (mirroring the input struct, which is omitted when empty).
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

    // Seed a fresh UUID-v7 id; a non-`Uuid` primary key (e.g. auto-increment) is
    // left `NotSet` for the database to assign.
    let pk_seed = match model.fields.iter().find(|f| f.is_pk) {
        Some(pk) if is_uuid(&pk.ty) => {
            let id = &pk.ident;
            quote! { __am.#id = ::sea_orm::ActiveValue::Set(::uuid::Uuid::now_v7()); }
        }
        _ => quote! {},
    };

    let create = &model.create_input_ident;
    quote! {
        impl ::nestrs_database::CreateModel<Entity> for #create {
            fn into_active_model(self) -> ActiveModel {
                let mut __am = <ActiveModel as ::core::default::Default>::default();
                #pk_seed
                #(#setters)*
                __am
            }
        }
    }
}

/// `Update<Name>Input::apply_to` — overwrite the update columns on an already
/// loaded `ActiveModel` (from the route-model `Bind`). Emitted only when there is
/// at least one update column.
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

    let update = &model.update_input_ident;
    quote! {
        impl ::nestrs_database::UpdateModel<Entity> for #update {
            fn apply_to(self, mut __am: ActiveModel) -> ActiveModel {
                #(#setters)*
                __am
            }
        }
    }
}
