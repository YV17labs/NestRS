//! Lifecycle hooks emitted by `#[expose(..., soft_delete)]` and
//! `#[expose(..., timestamps)]`.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Ident, Type};

use crate::attr::ResourceModel;

// `attr::parse` has validated the `deleted_at` / `created_at` / `updated_at` columns.
pub(crate) fn emit(model: &ResourceModel) -> TokenStream2 {
    let mut blocks = Vec::new();
    if let Some(column) = &model.deleted_at {
        blocks.push(emit_soft_deletable(column));
    }
    if model.timestamps {
        blocks.push(emit_timestamps());
    }
    quote! { #(#blocks)* }
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

fn emit_timestamps() -> TokenStream2 {
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
                let now: ::nest_rs_seaorm::sea_orm::prelude::DateTimeWithTimeZone =
                    ::nest_rs_seaorm::__private::chrono::Utc::now().fixed_offset();
                if insert {
                    self.created_at = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(now);
                }
                self.updated_at = ::nest_rs_seaorm::sea_orm::ActiveValue::Set(now);
                ::core::result::Result::Ok(self)
            }
        }
    }
}

pub(crate) fn is_option_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Option")
    )
}
