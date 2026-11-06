//! Emit `WireModel`: the round trip a masked reply runs on the entity's rows.
//!
//! **Each column goes through its own serde impls**, so the `Model` needs none
//! and crosses no wire but as its exposed type (`refused_wires`). An exposed
//! column is read back from the wire object; any other is rebuilt from its
//! placeholder — its type's `Default` in a narrow set (empty string, `None`,
//! `false`, `0`), a `Secret` taking its value's, or the `#[wire_default]` written
//! on it — and a hidden column with none, or a hidden primary key, is refused at
//! its field. Every column the rebuild invents is named by `is_placeholdered`,
//! from the same walk, so the mask never decides a rule on a placeholder; every
//! exposed column is keyed by `wire_key`, so the mask knows one an object left
//! out, and `key_column` reads a key back to the column a field grant names.

use nest_rs_codegen::__private::{site, type_label};
use nest_rs_codegen::Refusals;
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Ident, LitStr, Type};

use crate::attr::{ResourceField, ResourceModel, holds_secret, secret_inner};

/// The value a masked reply rebuilds the hidden column `field` with, or the
/// refusal naming what to write when none exists.
fn placeholder(field: &ResourceField) -> syn::Result<TokenStream> {
    let ty = &field.ty;
    let default = quote!(<#ty as ::core::default::Default>::default());
    let refusal = if field.is_pk {
        "is the primary key, which a masked reply reads off the wire and never invents — \
         `#[expose]` it"
    } else {
        match &field.wire_default {
            Some(Some(expr)) => return Ok(quote!(#expr)),
            Some(None) => return Ok(default),
            // A `Secret` defaults to its value's `Default`, so it takes the same test.
            None if defaults_to_a_placeholder(secret_inner(ty).unwrap_or(ty)) => {
                return Ok(default);
            }
            // A secret is never exposed, so its sentence offers the two ways it has.
            None if holds_secret(ty) => {
                "is not exposed, and a masked reply must rebuild it — write `#[wire_default]` \
                 (its type's `Default`) or `#[wire_default(expr)]`: a `Secret` never crosses \
                 the wire"
            }
            None => {
                "is not exposed, and a masked reply must rebuild it — write `#[wire_default]` \
                 (its type's `Default`), `#[wire_default(expr)]`, or `#[expose]` it"
            }
        }
    };
    Err(syn::Error::new_spanned(
        &field.ident,
        format!(
            "{}: `{}: {}` {refusal}",
            site("expose", None),
            field.key().value(),
            type_label(ty),
        ),
    ))
}

/// The types whose `Default` is a placeholder no column value is mistaken for.
const PLACEHOLDER_TYPES: [&str; 17] = [
    "String", "Option", "bool", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32",
    "u64", "u128", "usize", "f32", "f64",
];

/// Whether `ty` is one of [`PLACEHOLDER_TYPES`], read by its last segment, so
/// an alias of one takes `#[wire_default]`.
fn defaults_to_a_placeholder(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|last| {
        PLACEHOLDER_TYPES.iter().any(|name| last.ident == name)
    }))
}

/// A parameter of an emitted method, underscored when its body never reads it.
fn param(name: &str, read: bool) -> Ident {
    let unread = if read { "" } else { "_" };
    Ident::new(&format!("{unread}{name}"), Span::call_site())
}

pub(crate) fn emit(model: &ResourceModel) -> TokenStream {
    let source = &model.source_ident;
    let mut refusals = Refusals::default();
    let mut reads = Vec::new();
    let mut exposed: Vec<&Ident> = Vec::new();
    let mut wire_keys: Vec<LitStr> = Vec::new();
    let mut keyed: Vec<&Ident> = Vec::new();
    let mut keys: Vec<LitStr> = Vec::new();
    let mut invented: Vec<&Ident> = Vec::new();
    // A relation is no column of the row: the `Model` carries none.
    for field in model.fields.iter().filter(|field| field.relation.is_none()) {
        let ident = &field.ident;
        if field.read {
            let (ty, key) = (&field.ty, field.key());
            reads.push(quote! {
                #ident: ::nest_rs_authz::__private::wire_column::<#ty>(wire, #key)?
            });
            if let Some(column) = &field.column {
                keyed.push(column);
                keys.push(key.clone());
            }
            exposed.push(ident);
            wire_keys.push(key);
        } else if let Some(value) = refusals.handle(placeholder(field)) {
            reads.push(quote!(#ident: #value));
            if let Some(column) = &field.column {
                invented.push(column);
            }
        }
    }
    let refused = refusals
        .finish()
        .err()
        .map(|refused| refused.to_compile_error());

    let wire_ty = quote! {
        ::nest_rs_seaorm::__private::serde_json::Map<
            ::std::string::String,
            ::nest_rs_seaorm::__private::serde_json::Value,
        >
    };
    let error_ty = quote!(::nest_rs_seaorm::__private::serde_json::Error);
    let model_param = param("model", !exposed.is_empty());
    let to_wire = if exposed.is_empty() {
        quote!(::core::result::Result::Ok(<#wire_ty>::new()))
    } else {
        let capacity = exposed.len();
        quote! {
            let mut wire = <#wire_ty>::with_capacity(#capacity);
            #(
                wire.insert(
                    ::std::string::String::from(#wire_keys),
                    ::nest_rs_seaorm::__private::serde_json::to_value(&model.#exposed)?,
                );
            )*
            ::core::result::Result::Ok(wire)
        }
    };
    let wire_param = param("wire", refused.is_none() && !wire_keys.is_empty());
    // A refused column has no value to rebuild with; its compile error stands,
    // and the stub keeps the masks from adding one of their own.
    let from_wire = if refused.is_some() {
        quote!(::core::unreachable!())
    } else {
        quote!(::core::result::Result::Ok(#source { #(#reads),* }))
    };
    let placeholdered_param = param("column", !invented.is_empty());
    let is_placeholdered = if invented.is_empty() {
        quote!(false)
    } else {
        quote!(::core::matches!(column, #(Column::#invented)|*))
    };
    // An `if` chain, not a `match`: a `_` arm would be unreachable when every
    // column is exposed.
    let keyed_param = param("column", !keyed.is_empty());
    let column_iden = quote!(<Column as ::nest_rs_seaorm::sea_orm::IdenStatic>::as_str);
    let key_param = param("key", !keyed.is_empty());
    quote! {
        #refused

        impl ::nest_rs_authz::__private::WireModel for Entity {
            const WIRE_KEYS: &'static [&'static str] = &[#(#wire_keys),*];

            fn to_wire(
                #model_param: &Self::Model,
            ) -> ::core::result::Result<#wire_ty, #error_ty> {
                #to_wire
            }

            fn from_wire(
                #wire_param: &#wire_ty,
            ) -> ::core::result::Result<Self::Model, #error_ty> {
                #from_wire
            }

            fn is_placeholdered(#placeholdered_param: Self::Column) -> bool {
                #is_placeholdered
            }

            fn wire_key(#keyed_param: Self::Column) -> ::core::option::Option<&'static str> {
                #(
                    if ::core::matches!(column, Column::#keyed) {
                        return ::core::option::Option::Some(#keys);
                    }
                )*
                ::core::option::Option::None
            }

            fn key_column(#key_param: &str) -> ::core::option::Option<&'static str> {
                #(
                    if key == #keys {
                        return ::core::option::Option::Some(#column_iden(&Column::#keyed));
                    }
                )*
                ::core::option::Option::None
            }
        }
    }
}
