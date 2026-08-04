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
        blocks.push(emit_soft_delete_registration(model));
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

/// Pair the entity's flag with the service's override, for the boot audit.
///
/// The flag alone is half a feature: without
/// `CrudService::soft_delete_column` the column exists, `SoftDeletable` is
/// implemented, and `DELETE` still erases the row — answering `204` exactly as a
/// successful tombstone does. `#[expose(service = …)]` already names the service,
/// so this is the one site where both halves are in scope; the audit
/// (`nest_rs_seaorm::SoftDeleteAudit`) reads the pair at boot.
///
/// No `service` ⇒ no entry: the pair cannot be formed, and requiring `service`
/// for `soft_delete` would reject a read-only exposure that has no service at
/// all.
fn emit_soft_delete_registration(model: &ResourceModel) -> TokenStream2 {
    let Some(service) = &model.service else {
        return TokenStream2::new();
    };
    quote! {
        ::nest_rs_seaorm::inventory::submit! {
            ::nest_rs_seaorm::SoftDeleteRegistration {
                // Through the service, not `table_name()` directly: `entity_name`
                // is what every `nest_rs_seaorm::service` log carries, so the
                // refusal names the entity the reader will grep for.
                entity: || <#service as ::nest_rs_seaorm::CrudService>::entity_name(),
                service: || ::core::any::type_name::<#service>(),
                tombstones: || <#service as ::nest_rs_seaorm::CrudService>::soft_delete_column()
                    .is_some(),
            }
        }
    }
}

fn emit_timestamps() -> TokenStream2 {
    quote! {
        #[::nest_rs_resource::async_trait]
        impl ::nest_rs_resource::sea_orm::ActiveModelBehavior for ActiveModel {
            async fn before_save<C>(
                mut self,
                _db: &C,
                insert: bool,
            ) -> ::core::result::Result<Self, ::nest_rs_resource::sea_orm::DbErr>
            where
                C: ::nest_rs_resource::sea_orm::ConnectionTrait,
            {
                let now: ::nest_rs_resource::sea_orm::prelude::DateTimeWithTimeZone =
                    ::nest_rs_resource::chrono::Utc::now().fixed_offset();
                if insert {
                    self.created_at = ::nest_rs_resource::sea_orm::ActiveValue::Set(now);
                }
                self.updated_at = ::nest_rs_resource::sea_orm::ActiveValue::Set(now);
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
