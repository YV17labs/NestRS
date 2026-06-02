//! Emit `WireModelDefaults` for `#[expose(skip)]` scalar columns.

use quote::quote;
use syn::{Type, TypePath};

use crate::attr::{is_uuid, ResourceField, ResourceModel};

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
    if is_uuid(ty) {
        return None;
    }
    let last = match ty {
        Type::Path(tp) => tp.path.segments.last()?.ident.to_string(),
        _ => return None,
    };
    Some(match last.as_str() {
        "String" => quote! {
            map.entry(::std::string::String::from(stringify!(#key)))
                .or_insert_with(|| ::serde_json::Value::String(String::new()));
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
        _ => return None,
    })
}

pub fn emit(model: &ResourceModel) -> proc_macro2::TokenStream {
    let entries = model
        .fields
        .iter()
        .filter_map(default_value_tokens)
        .collect::<Vec<_>>();
    quote! {
        impl ::nestrs_resource::WireModelDefaults for Entity {
            fn fill_wire_defaults(map: &mut ::serde_json::Map<String, ::serde_json::Value>) {
                #(#entries)*
            }
        }
    }
}
