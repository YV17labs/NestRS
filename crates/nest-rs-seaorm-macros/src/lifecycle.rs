//! The lifecycle `#[expose]` emits: every exposed entity's column defaults —
//! a `Uuid` key's v7 UUID and each column's `default` — registered for the
//! insert to fill what a row left unset; `soft_delete`'s registration; and
//! under `timestamps` the entity's `ActiveModelBehavior`, which stamps
//! `created_at` on insert and, when the entity has one, `updated_at` on every
//! save. Without `timestamps` the entity's behaviour, and every hook in it, is
//! its own.

use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::{Ident, Type};

use crate::attr::{ResourceModel, is_uuid};

// `attr::parse` has validated the `deleted_at` / `created_at` / `updated_at` columns.
pub(crate) fn emit(model: &ResourceModel) -> TokenStream2 {
    let column_defaults = emit_column_defaults(model);
    let soft_deletable = model.deleted_at.as_ref().map(emit_soft_deletable);
    let timestamps = model.timestamps.then(|| emit_timestamps(model));
    quote! {
        #column_defaults
        #soft_deletable
        #timestamps
    }
}

/// The registration `Repo`'s inserts consult, keyed by the `ActiveModel` its
/// fill downcasts to; none for an entity with nothing to default.
fn emit_column_defaults(model: &ResourceModel) -> TokenStream2 {
    let mut seeds = Vec::new();
    if let Some(pk) = model.fields.iter().find(|f| f.is_pk && is_uuid(&f.ty)) {
        seeds.push(seed(
            &pk.ident,
            quote!(::nest_rs_seaorm::__private::uuid::Uuid::now_v7()),
        ));
    }
    seeds.extend(
        model
            .fields
            .iter()
            .filter_map(|f| Some(seed(&f.ident, f.default.as_ref()?))),
    );
    if seeds.is_empty() {
        return quote! {};
    }
    quote! {
        ::nest_rs_seaorm::__private::inventory::submit! {
            ::nest_rs_seaorm::__private::ColumnDefaultRegistration {
                active_model: || ::core::any::TypeId::of::<ActiveModel>(),
                fill: |__active| {
                    if let ::core::option::Option::Some(__active) = __active.downcast_mut::<ActiveModel>() {
                        #(#seeds)*
                    }
                },
            }
        }
    }
}

/// The trait, its seal, and the registration every `Repo` read consults: an
/// entity declared `soft_delete` hides its tombstones whether or not it names
/// a service.
fn emit_soft_deletable(column: &Ident) -> TokenStream2 {
    quote! {
        impl ::nest_rs_seaorm::__private::SoftDeleteDeclared for Entity {}

        impl ::nest_rs_seaorm::SoftDeletable for Entity {
            fn deleted_at_column() -> Column {
                Column::#column
            }
        }

        ::nest_rs_seaorm::__private::inventory::submit! {
            ::nest_rs_seaorm::__private::SoftDeleteRegistration {
                entity: || ::core::any::TypeId::of::<Entity>(),
                column: || <Column as ::nest_rs_seaorm::sea_orm::IdenStatic>::as_str(
                    &Column::#column,
                ),
            }
        }
    }
}

/// `timestamps`' `ActiveModelBehavior`, so a hand-written one beside it is a
/// conflicting impl. One instant per save: a fresh row's `created_at` equals
/// its `updated_at`.
fn emit_timestamps(model: &ResourceModel) -> TokenStream2 {
    let updated_at = model
        .updated_at
        .then(|| quote! { self.updated_at = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(__now); });
    quote! {
        #[::nest_rs_seaorm::__private::async_trait]
        impl ::nest_rs_seaorm::sea_orm::ActiveModelBehavior for ActiveModel {
            async fn before_save<C>(
                mut self,
                _db: &C,
                insert: bool,
            ) -> ::core::result::Result<Self, ::nest_rs_seaorm::sea_orm::DbErr>
            where
                C: ::nest_rs_seaorm::sea_orm::ConnectionTrait,
            {
                let __now = ::nest_rs_seaorm::now();
                if insert {
                    self.created_at = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(__now);
                }
                #updated_at
                ::core::result::Result::Ok(self)
            }
        }
    }
}

/// `column = value` on the registration's `ActiveModel` when it leaves
/// `column` unset.
fn seed(column: &Ident, value: impl ToTokens) -> TokenStream2 {
    quote! {
        if __active.#column.is_not_set() {
            __active.#column = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(#value);
        }
    }
}

pub(crate) fn is_option_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Option")
    )
}
