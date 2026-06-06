//! Emit `WireModelDefaults` for `#[expose(skip)]` scalar columns.
//!
//! These placeholders feed `Model::deserialize` only so `Ability::mask` /
//! `mask_many` can run against the reconstructed `Model`; `retain_wire_keys`
//! strips them again before the body hits the network.
//!
//! **The type set is narrow on purpose.** `Ability::can(action, &model)` runs
//! per row — a rule predicated on a skipped column would compare the placeholder
//! against the real value, silently filtering rows. So only types where the
//! placeholder is structurally distinguishable (empty string, null, false, 0)
//! get a default. For `Uuid`, timestamps, `Decimal`, custom enums, etc., emit
//! nothing — the shaper fails `wire_to_model` with 500 unless the user
//! hand-writes `impl WireModelDefaults for Entity` with an audited placeholder.

use quote::quote;
use syn::Type;

use crate::attr::{ResourceField, ResourceModel};

fn default_value_tokens(field: &ResourceField) -> Option<proc_macro2::TokenStream> {
    if !field.skip || field.is_pk || field.relation.is_some() {
        return None;
    }
    let key = &field.ident;
    let ty = &field.ty;
    let last = match ty {
        Type::Path(tp) => tp.path.segments.last()?.ident.to_string(),
        _ => return None,
    };
    Some(match last.as_str() {
        "String" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert_with(|| ::serde_json::Value::String(::std::string::String::new()));
        },
        "Option" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::serde_json::Value::Null);
        },
        "bool" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::serde_json::Value::Bool(false));
        },
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::serde_json::json!(0));
        },
        // See the module header — emit nothing, fail closed.
        _ => return None,
    })
}

pub fn emit(model: &ResourceModel) -> proc_macro2::TokenStream {
    let entries = model
        .fields
        .iter()
        .filter_map(default_value_tokens)
        .collect::<Vec<_>>();
    // `_map` keeps `#![deny(unused_variables)]` happy when no scalar emits a default.
    let param = if entries.is_empty() {
        quote!(_map)
    } else {
        quote!(map)
    };
    quote! {
        impl ::nest_rs_resource::WireModelDefaults for Entity {
            fn fill_wire_defaults(
                #param: &mut ::serde_json::Map<::std::string::String, ::serde_json::Value>,
            ) {
                #(#entries)*
            }
        }
    }
}
