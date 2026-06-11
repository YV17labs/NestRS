//! Lifecycle hooks emitted by `#[expose(..., soft_delete)]` and
//! `#[expose(..., timestamps)]`.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Type;

use crate::attr::ResourceModel;

// `parse` (attr.rs) has already validated that the conventional `deleted_at` /
// `created_at` / `updated_at` columns exist and have the right shape, so the
// emitters here rely on those fixed names rather than re-discovering them.
pub fn emit(model: &ResourceModel) -> TokenStream2 {
    let mut blocks = Vec::new();
    if model.soft_delete {
        blocks.push(emit_soft_deletable());
    }
    if model.timestamps {
        blocks.push(emit_timestamps());
    }
    quote! { #(#blocks)* }
}

fn emit_soft_deletable() -> TokenStream2 {
    quote! {
        impl ::nest_rs_seaorm::SoftDeletable for Entity {
            fn deleted_at_column() -> Column {
                Column::DeletedAt
            }
        }
    }
}

fn emit_timestamps() -> TokenStream2 {
    quote! {
        #[::async_trait::async_trait]
        impl ::sea_orm::ActiveModelBehavior for ActiveModel {
            async fn before_save<C>(
                mut self,
                _db: &C,
                insert: bool,
            ) -> ::core::result::Result<Self, ::sea_orm::DbErr>
            where
                C: ::sea_orm::ConnectionTrait,
            {
                let now: ::sea_orm::prelude::DateTimeWithTimeZone =
                    ::chrono::Utc::now().fixed_offset();
                if insert {
                    self.created_at = ::sea_orm::ActiveValue::Set(now);
                }
                self.updated_at = ::sea_orm::ActiveValue::Set(now);
                ::core::result::Result::Ok(self)
            }
        }
    }
}

/// True when the type is `Option<…>`.
pub(crate) fn is_option_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Option")
    )
}
