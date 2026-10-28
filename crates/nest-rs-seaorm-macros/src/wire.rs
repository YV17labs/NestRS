//! Emit `WireModelDefaults` for unexposed scalar columns (no `#[expose]`).
//!
//! **The type set is narrow on purpose**: only distinguishable placeholders
//! (empty string, null, false, 0) are emitted, a `Secret` column taking its
//! value's; any other type fails closed (500) unless `#[wire_default]`. Every
//! column the fill may invent is named by `is_placeholdered`, from the same
//! walk, so the mask never decides a rule on a placeholder; every exposed column
//! is keyed by `wire_key`, so the mask knows one an object left out.
//!
//! **A secret column is filled with what its value type reads**, and over any
//! value the object carries: a raw `Model` serializes a `Secret` as
//! `"<redacted>"`, which a byte-like secret's value type cannot read back.

use nest_rs_codegen::__private::generic_args;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, LitStr, Type};

use crate::attr::{ResourceField, ResourceModel, holds_secret, secret_inner};

fn default_value_tokens(field: &ResourceField) -> Option<TokenStream> {
    if field.read || field.is_pk || field.relation.is_some() {
        return None;
    }
    let key = field.key();
    let ty = &field.ty;
    let secret = holds_secret(ty);
    let value = match &field.wire_default {
        None => return placeholder(ty).map(|value| fill(&key, secret, &value)),
        Some(Some(expr)) => quote!(#expr),
        Some(None) => quote!(<#ty as ::core::default::Default>::default()),
    };
    let serialized = if secret {
        let held = held(ty, quote!(&__secret));
        quote! {{
            let __secret: #ty = #value;
            ::nest_rs_seaorm::__private::serde_json::to_value(#held)
        }}
    } else {
        quote!(::nest_rs_seaorm::__private::serde_json::to_value(#value))
    };
    let set = fill(&key, secret, &quote!(__v));
    // Skip-on-error, never `expect`: a missing key fails the masker closed
    // (500) instead of panicking on the request path.
    Some(quote! {
        if let ::core::result::Result::Ok(__v) = #serialized {
            #set
        }
    })
}

/// Write `value` under `key`: over what the object carries for a secret
/// column, beside what it carries for any other.
fn fill(key: &LitStr, secret: bool, value: &TokenStream) -> TokenStream {
    if secret {
        quote!(map.insert(::std::string::String::from(#key), #value);)
    } else {
        quote!(map.entry(::std::string::String::from(#key)).or_insert(#value);)
    }
}

/// `value`, a `&ty`, read through each `Secret` in `ty` — bare or in an
/// `Option` — so it serializes as the column's value, not the placeholder.
fn held(ty: &Type, value: TokenStream) -> TokenStream {
    if let Some(inner) = secret_inner(ty) {
        return held(inner, quote!((#value).expose_secret()));
    }
    match generic_args(ty) {
        Some((ident, args)) if ident == "Option" && args.len() == 1 => {
            let inner = held(args[0], quote!(__x));
            quote! {
                ::core::option::Option::map(::core::option::Option::as_ref(#value), |__x| #inner)
            }
        }
        _ => value,
    }
}

/// The placeholder for a column of type `ty`, from the narrow set; a
/// `Secret<T>` takes `T`'s, which it deserializes from.
fn placeholder(ty: &Type) -> Option<TokenStream> {
    if let Some(inner) = secret_inner(ty) {
        return placeholder(inner);
    }
    let last = match ty {
        Type::Path(tp) => tp.path.segments.last()?.ident.to_string(),
        _ => return None,
    };
    Some(match last.as_str() {
        "String" => quote! {
            ::nest_rs_seaorm::__private::serde_json::Value::String(::std::string::String::new())
        },
        "Option" => quote!(::nest_rs_seaorm::__private::serde_json::Value::Null),
        "bool" => quote!(::nest_rs_seaorm::__private::serde_json::Value::Bool(false)),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" | "f32" | "f64" => quote!(::nest_rs_seaorm::__private::serde_json::json!(0)),
        _ => return None,
    })
}

pub(crate) fn emit(model: &ResourceModel) -> TokenStream {
    let (filled, entries): (Vec<&ResourceField>, Vec<_>) = model
        .fields
        .iter()
        .filter_map(|field| Some((field, default_value_tokens(field)?)))
        .unzip();
    // `_map` keeps `#![deny(unused_variables)]` happy when no scalar emits a default.
    let param = if entries.is_empty() {
        quote!(_map)
    } else {
        quote!(map)
    };
    let invented: Vec<_> = filled
        .iter()
        .filter_map(|field| field.column.as_ref())
        .collect();
    let is_placeholdered = if invented.is_empty() {
        quote!()
    } else {
        quote! {
            fn is_placeholdered(
                column: <Self as ::nest_rs_seaorm::sea_orm::EntityTrait>::Column,
            ) -> bool {
                ::core::matches!(column, #(Column::#invented)|*)
            }
        }
    };
    // Holds only while the wire DTO serializes each field under its ident, unrenamed.
    let exposed: Vec<&ResourceField> = model
        .fields
        .iter()
        .filter(|f| f.in_output_struct())
        .collect();
    let wire_keys = exposed.iter().map(|field| field.key());
    // An `if` chain, not a `match`: a `_` arm would be unreachable when every
    // column is exposed.
    let (keyed, keys): (Vec<&Ident>, Vec<LitStr>) = exposed
        .iter()
        .filter_map(|field| Some((field.column.as_ref()?, field.key())))
        .unzip();
    // The trait's default guesses from column names; an `#[expose]`d entity
    // always answers from its fields, even with none to key.
    let wire_key = if keyed.is_empty() {
        quote! {
            fn wire_key(
                _column: <Self as ::nest_rs_seaorm::sea_orm::EntityTrait>::Column,
            ) -> ::core::option::Option<&'static str> {
                ::core::option::Option::None
            }
        }
    } else {
        quote! {
            fn wire_key(
                column: <Self as ::nest_rs_seaorm::sea_orm::EntityTrait>::Column,
            ) -> ::core::option::Option<&'static str> {
                #(
                    if ::core::matches!(column, Column::#keyed) {
                        return ::core::option::Option::Some(#keys);
                    }
                )*
                ::core::option::Option::None
            }
        }
    };
    quote! {
        impl ::nest_rs_authz::WireModelDefaults for Entity {
            fn fill_wire_defaults(
                #param: &mut ::nest_rs_seaorm::__private::serde_json::Map<::std::string::String, ::nest_rs_seaorm::__private::serde_json::Value>,
            ) {
                #(#entries)*
            }

            fn wire_keys() -> ::core::option::Option<&'static [&'static str]> {
                ::core::option::Option::Some(&[#(#wire_keys),*])
            }

            #is_placeholdered

            #wire_key
        }
    }
}
