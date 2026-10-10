//! Emit `WireModelDefaults` for unexposed scalar columns (no `#[expose]`).
//!
//! **The type set is narrow on purpose**: `Ability::can` runs per row on the
//! reconstructed `Model`, so a rule on a hidden column would compare against the
//! placeholder. Only distinguishable placeholders (empty string, null, false, 0)
//! are emitted; any other type fails closed (500) unless `#[wire_default]`.

use quote::quote;
use syn::Type;

use crate::attr::{ResourceField, ResourceModel};

fn default_value_tokens(field: &ResourceField) -> Option<proc_macro2::TokenStream> {
    if field.read || field.is_pk || field.relation.is_some() {
        return None;
    }
    let key = &field.ident;
    let ty = &field.ty;
    if let Some(default) = &field.wire_default {
        let value = match default {
            Some(expr) => quote!(#expr),
            None => quote!(<#ty as ::core::default::Default>::default()),
        };
        // Skip-on-error, never `expect`: a missing key fails the masker closed
        // (500) instead of panicking on the request path.
        return Some(quote! {
            if let ::core::result::Result::Ok(__v) =
                ::nest_rs_resource::serde_json::to_value(#value)
            {
                map.entry(::std::string::String::from(stringify!(#key))).or_insert(__v);
            }
        });
    }
    let last = match ty {
        Type::Path(tp) => tp.path.segments.last()?.ident.to_string(),
        _ => return None,
    };
    Some(match last.as_str() {
        "String" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert_with(|| ::nest_rs_resource::serde_json::Value::String(::std::string::String::new()));
        },
        "Option" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::nest_rs_resource::serde_json::Value::Null);
        },
        "bool" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::nest_rs_resource::serde_json::Value::Bool(false));
        },
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert(::nest_rs_resource::serde_json::json!(0));
        },
        _ => return None,
    })
}

pub(crate) fn emit(model: &ResourceModel) -> proc_macro2::TokenStream {
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
    // Holds only while the wire DTO serializes each field under its ident, unrenamed.
    let wire_keys = model
        .fields
        .iter()
        .filter(|f| f.in_output_struct())
        .map(|f| {
            let key = &f.ident;
            quote! { stringify!(#key) }
        });
    quote! {
        impl ::nest_rs_authz::WireModelDefaults for Entity {
            fn fill_wire_defaults(
                #param: &mut ::nest_rs_resource::serde_json::Map<::std::string::String, ::nest_rs_resource::serde_json::Value>,
            ) {
                #(#entries)*
            }

            fn wire_keys() -> ::core::option::Option<&'static [&'static str]> {
                ::core::option::Option::Some(&[#(#wire_keys),*])
            }
        }
    }
}
