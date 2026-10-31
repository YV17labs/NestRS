//! Emit `WireModel`: the round trip a masked reply runs on the entity's rows.
//!
//! **Each column goes through its own serde impls**, so the `Model` derives
//! none and crosses no wire but as its exposed type. An exposed column is read
//! back from the wire object; any other is rebuilt from its placeholder — its
//! type's `Default` in a narrow set (empty string, `None`, `false`, `0`), a
//! `Secret` taking its value's, or the `#[wire_default]` written on it — and a
//! hidden column with none, or a hidden primary key, is refused at its field.
//! Every column the rebuild invents is named by `is_placeholdered`, from the
//! same walk, so the mask never decides a rule on a placeholder; every exposed
//! column is keyed by `wire_key`, so the mask knows one an object left out.

use nest_rs_codegen::__private::{site, type_label};
use nest_rs_codegen::Refusals;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, LitStr, Type};

use crate::attr::{ResourceField, ResourceModel, holds_secret, secret_inner};

/// The value a masked reply rebuilds the hidden column `field` with, or the
/// refusal naming what to write when none exists.
fn placeholder(field: &ResourceField) -> syn::Result<TokenStream> {
    let ty = &field.ty;
    let column = field.key().value();
    if field.is_pk {
        return Err(syn::Error::new_spanned(
            &field.ident,
            format!(
                "{}: `{column}: {}` is the primary key, which a masked reply reads off the wire and \
                 never invents — `#[expose]` it",
                site("expose", None),
                type_label(ty),
            ),
        ));
    }
    match &field.wire_default {
        Some(Some(expr)) => return Ok(quote!(#expr)),
        Some(None) => return Ok(quote!(<#ty as ::core::default::Default>::default())),
        None => {}
    }
    // A `Secret` defaults to its value's `Default`, so it takes the same test.
    if defaults_to_a_placeholder(secret_inner(ty).unwrap_or(ty)) {
        return Ok(quote!(<#ty as ::core::default::Default>::default()));
    }
    // A secret is never exposed, so its sentence offers the two ways it has.
    let ways = if holds_secret(ty) {
        "`#[wire_default]` (its type's `Default`) or `#[wire_default(expr)]`: a `Secret` \
         never crosses the wire"
    } else {
        "`#[wire_default]` (its type's `Default`), `#[wire_default(expr)]`, or `#[expose]` it"
    };
    Err(syn::Error::new_spanned(
        &field.ident,
        format!(
            "{}: `{column}: {}` is not exposed, and a masked reply must rebuild it — write {ways}",
            site("expose", None),
            type_label(ty),
        ),
    ))
}

/// Whether `ty`'s `Default` is a placeholder no column value is mistaken for:
/// read by its last segment, so an alias of one takes `#[wire_default]`.
fn defaults_to_a_placeholder(ty: &Type) -> bool {
    let Type::Path(tp) = ty else {
        return false;
    };
    tp.path.segments.last().is_some_and(|last| {
        matches!(
            last.ident.to_string().as_str(),
            "String"
                | "Option"
                | "bool"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "f32"
                | "f64"
        )
    })
}

pub(crate) fn emit(model: &ResourceModel) -> TokenStream {
    let source = &model.source_ident;
    let mut refusals = Refusals::default();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut wire_keys: Vec<LitStr> = Vec::new();
    let mut keyed: Vec<(&Ident, LitStr)> = Vec::new();
    let mut invented: Vec<&Ident> = Vec::new();
    // A relation is no column of the row: the `Model` carries none.
    for field in model.fields.iter().filter(|field| field.relation.is_none()) {
        let ident = &field.ident;
        let key = field.key();
        if field.read {
            let ty = &field.ty;
            reads.push(quote! {
                #ident: ::nest_rs_authz::__private::wire_column::<#ty>(wire, #key)?
            });
            writes.push(quote! {
                wire.insert(
                    ::std::string::String::from(#key),
                    ::nest_rs_seaorm::__private::serde_json::to_value(&model.#ident)?,
                );
            });
            if let Some(column) = &field.column {
                keyed.push((column, key.clone()));
            }
            wire_keys.push(key);
            continue;
        }
        if let Some(value) = refusals.handle(placeholder(field)) {
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
    // A refused column has no value to rebuild with; its compile error stands,
    // and the stub keeps the masks from adding one of their own.
    let from_wire = if refused.is_some() {
        quote! {
            fn from_wire(
                _wire: &#wire_ty,
            ) -> ::core::result::Result<Self::Model, #error_ty> {
                ::core::unreachable!()
            }
        }
    } else {
        let wire = if wire_keys.is_empty() {
            quote!(_wire)
        } else {
            quote!(wire)
        };
        quote! {
            fn from_wire(
                #wire: &#wire_ty,
            ) -> ::core::result::Result<Self::Model, #error_ty> {
                ::core::result::Result::Ok(#source { #(#reads),* })
            }
        }
    };
    let capacity = writes.len();
    let (model_param, to_wire_body) = if writes.is_empty() {
        (
            quote!(_model),
            quote!(::core::result::Result::Ok(<#wire_ty>::new())),
        )
    } else {
        (
            quote!(model),
            quote! {
                let mut wire = <#wire_ty>::with_capacity(#capacity);
                #(#writes)*
                ::core::result::Result::Ok(wire)
            },
        )
    };
    let is_placeholdered = if invented.is_empty() {
        quote! {
            fn is_placeholdered(
                _column: <Self as ::nest_rs_seaorm::sea_orm::EntityTrait>::Column,
            ) -> bool {
                false
            }
        }
    } else {
        quote! {
            fn is_placeholdered(
                column: <Self as ::nest_rs_seaorm::sea_orm::EntityTrait>::Column,
            ) -> bool {
                ::core::matches!(column, #(Column::#invented)|*)
            }
        }
    };
    // An `if` chain, not a `match`: a `_` arm would be unreachable when every
    // column is exposed.
    let wire_key = if keyed.is_empty() {
        quote! {
            fn wire_key(
                _column: <Self as ::nest_rs_seaorm::sea_orm::EntityTrait>::Column,
            ) -> ::core::option::Option<&'static str> {
                ::core::option::Option::None
            }
        }
    } else {
        let (keyed, keys): (Vec<&Ident>, Vec<LitStr>) = keyed.into_iter().unzip();
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
        #refused

        impl ::nest_rs_authz::__private::WireModel for Entity {
            const WIRE_KEYS: &'static [&'static str] = &[#(#wire_keys),*];

            fn to_wire(
                #model_param: &Self::Model,
            ) -> ::core::result::Result<#wire_ty, #error_ty> {
                #to_wire_body
            }

            #from_wire

            #is_placeholdered

            #wire_key
        }
    }
}
