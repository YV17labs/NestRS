//! `#[input]` — shorthand attribute for input DTOs. Expands to
//! `#[derive(::serde::Deserialize, ::validator::Validate, ::schemars::JsonSchema)]`
//! plus `#[serde(deny_unknown_fields)]`, so a payload carrying an unknown field
//! (e.g. `is_admin: true`) is rejected at parse time instead of silently
//! ignored. The derives are appended to any existing `#[derive(...)]` on the
//! struct so the user can still add `Debug`, `Clone`, etc.
//!
//! `JsonSchema` is included because it is not optional in practice: `#[routes]`
//! documents every `Json<T>` / `Query<T>` argument in the OpenAPI document, so a
//! DTO without it fails to compile with a trait-bound error pointing at
//! `schema_of` rather than at the missing derive. Carrying it here is the
//! decorator doing its job; the alternative was every DTO repeating a derive the
//! shorthand exists to absorb.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Item, parse_macro_input};

pub(crate) fn input(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[input] takes no arguments",
        )
        .to_compile_error()
        .into();
    }

    let item = parse_macro_input!(input as Item);
    let Item::Struct(item) = item else {
        return syn::Error::new_spanned(item, "#[input] may only be applied to a struct")
            .to_compile_error()
            .into();
    };

    quote! {
        #[derive(::serde::Deserialize, ::validator::Validate, ::schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        #item
    }
    .into()
}
