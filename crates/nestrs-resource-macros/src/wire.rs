//! Emit `WireModelDefaults` for `#[expose(skip)]` scalar columns.
//!
//! These defaults are placeholders the response shaper feeds to
//! `Model::deserialize` only so the reconstructed `Model` can run through
//! `Ability::mask` / `mask_many`. The masker strips every skipped key from
//! the wire body on the way back out via `retain_wire_keys`, so a
//! placeholder's value never reaches the network.
//!
//! **Why the supported type set is narrow.** `Ability::mask_many` calls
//! `Ability::can(action, &model)` per row — a rule predicated on a skipped
//! column would compare the *placeholder* against the real value, silently
//! filtering legitimate rows. So this macro only emits a default when the
//! placeholder is structurally distinguishable from a real value (empty
//! string, `null`, `false`, `0`) — types where a predicate match is
//! coincidence rather than misleading.
//!
//! For `Uuid`, timestamps, `Decimal`, custom enums, and any other type the
//! caller marks `#[expose(skip)]`: emit no default. The shaper then fails
//! `wire_to_model` and returns `500` rather than running a predicate against
//! a fake. If you legitimately need to skip such a column, hand-write
//! `impl WireModelDefaults for Entity` next to the entity — picking a
//! placeholder you have audited against your policy's predicates.

use quote::quote;
use syn::{Type, TypePath};

use crate::attr::{ResourceField, ResourceModel};

fn is_relation(ty: &Type) -> bool {
    match ty {
        Type::Path(TypePath { path, .. }) => path
            .segments
            .last()
            .is_some_and(|s| matches!(s.ident.to_string().as_str(), "HasOne" | "HasMany")),
        _ => false,
    }
}

fn default_value_tokens(field: &ResourceField) -> Option<proc_macro2::TokenStream> {
    if !field.skip || field.is_pk || is_relation(&field.ty) {
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
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
        | "u128" | "usize" | "f32" | "f64" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::serde_json::json!(0));
        },
        // `Uuid`, `DateTime`, `Decimal`, custom enums, and anything else fall
        // here: emit no default, let `wire_to_model` fail closed with 500,
        // and require the user to hand-roll `WireModelDefaults` if they need
        // to skip such a column (auditing the placeholder against the
        // policy's predicates).
        _ => return None,
    })
}

pub fn emit(model: &ResourceModel) -> proc_macro2::TokenStream {
    let entries = model
        .fields
        .iter()
        .filter_map(default_value_tokens)
        .collect::<Vec<_>>();
    // When no skipped scalar emits a default, the body is empty — name the
    // unused parameter `_map` so user crates compiling with
    // `#![deny(unused_variables)]` are not broken by `#[expose]` on such
    // an entity.
    let param = if entries.is_empty() {
        quote!(_map)
    } else {
        quote!(map)
    };
    quote! {
        impl ::nestrs_resource::WireModelDefaults for Entity {
            fn fill_wire_defaults(
                #param: &mut ::serde_json::Map<::std::string::String, ::serde_json::Value>,
            ) {
                #(#entries)*
            }
        }
    }
}
