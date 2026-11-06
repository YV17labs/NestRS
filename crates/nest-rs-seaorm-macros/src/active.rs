//! Emit `CreateModel` / `UpdateModel` impls — the conversions `CrudService`'s
//! default `create`/`update` call. Only `input(create)` / `input(update)`
//! columns are set, and a `Uuid` key is seeded; every other column stays
//! `NotSet`, for the create to fill from what the caller's grants pin and the
//! insert from the entity's column defaults (`lifecycle.rs`). Each impl names
//! the field carrying each column it sets (`field_for`), and every entity
//! registers its table, whose unique keys the binding reads at boot, so a unique
//! conflict names the fields of the input a `#[crud]` wrote, a hand-written one's
//! included, and an update which of them a reply also shows (`reads_back`), which
//! a caller may carry back as they stand under a grant narrowing what it writes.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::attr::{ResourceField, ResourceModel, is_uuid};

pub(crate) fn emit(model: &ResourceModel) -> TokenStream2 {
    let create = emit_create(model);
    let update = emit_update(model);
    let registration = emit_registration();
    quote! {
        #create
        #update
        #registration
    }
}

/// The registration the binding reads the entity's unique keys by.
fn emit_registration() -> TokenStream2 {
    quote! {
        ::nest_rs_seaorm::__private::inventory::submit! {
            ::nest_rs_seaorm::__private::UniqueKeyRegistration {
                table: || ::nest_rs_seaorm::sea_orm::EntityName::table_name(
                    &<Entity as ::core::default::Default>::default(),
                ),
                schema: || ::nest_rs_seaorm::sea_orm::EntityName::schema_name(
                    &<Entity as ::core::default::Default>::default(),
                )
                .map(::std::borrow::ToOwned::to_owned),
            }
        }
    }
}

/// `field_for`, over the fields an input sets: each column's name, as the
/// table spells it, against the key serde reads the field under.
fn emit_field_for(fields: &[&ResourceField]) -> TokenStream2 {
    let pairs = fields.iter().filter_map(|f| {
        let column = f.column.as_ref()?;
        let key = f.key();
        Some(quote! {
            (<Column as ::nest_rs_seaorm::sea_orm::IdenStatic>::as_str(&Column::#column), #key)
        })
    });
    quote! {
        fn field_for(column: &str) -> ::core::option::Option<&'static str> {
            [#(#pairs),*]
                .into_iter()
                .find_map(|(__name, __field)| (__name == column).then_some(__field))
        }
    }
}

fn emit_create(model: &ResourceModel) -> TokenStream2 {
    let carried: Vec<&ResourceField> = model
        .fields
        .iter()
        .filter(|f| f.in_create && !f.is_pk)
        .collect();
    let setters: Vec<TokenStream2> = carried
        .iter()
        .map(|f| {
            let name = &f.ident;
            quote! { __am.#name = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(self.#name); }
        })
        .collect();
    if setters.is_empty() {
        return quote! {};
    }
    let field_for = emit_field_for(&carried);

    let pk_seed = match model.fields.iter().find(|f| f.is_pk) {
        Some(pk) if is_uuid(&pk.ty) => {
            let id = &pk.ident;
            quote! { __am.#id = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(::nest_rs_seaorm::__private::uuid::Uuid::now_v7()); }
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

            #field_for
        }
    }
}

fn emit_update(model: &ResourceModel) -> TokenStream2 {
    let carried: Vec<&ResourceField> = model
        .fields
        .iter()
        .filter(|f| f.in_update && !f.is_pk)
        .collect();
    let setters: Vec<TokenStream2> = carried
        .iter()
        .map(|f| {
            let name = &f.ident;
            quote! { __am.#name = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(self.#name); }
        })
        .collect();
    if setters.is_empty() {
        return quote! {};
    }
    let field_for = emit_field_for(&carried);
    let reads_back = emit_reads_back(&carried);

    let update = &model.update_ident;
    quote! {
        impl ::nest_rs_seaorm::UpdateModel<Entity> for #update {
            fn apply_to(self, mut __am: ActiveModel) -> ActiveModel {
                #(#setters)*
                __am
            }

            #field_for

            #reads_back
        }
    }
}

/// `reads_back`, over the fields an update sets, by each one's column as the table
/// spells it: `#[expose]` shows a field whatever else it says, `input(update)` among it.
fn emit_reads_back(fields: &[&ResourceField]) -> TokenStream2 {
    let shown = fields.iter().filter_map(|f| f.column.as_ref());
    quote! {
        fn reads_back(column: &str) -> bool {
            [#(<Column as ::nest_rs_seaorm::sea_orm::IdenStatic>::as_str(&Column::#shown)),*]
                .contains(&column)
        }
    }
}
