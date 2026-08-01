//! `#[input]` — the wire-DTO shorthand. Carries `Serialize`, `Deserialize`,
//! `Validate` and `JsonSchema`, each routed through `nest_rs_core` with its own
//! `crate = ` override, plus `#[serde(deny_unknown_fields)]` so a payload
//! carrying an unknown field (e.g. `is_admin: true`) is rejected at parse time
//! instead of silently ignored. The derives are appended to any existing
//! `#[derive(...)]` so the user can still add `Debug`, `Clone`, etc.
//!
//! The routing is the point: a derive expands against the *call site's*
//! prelude, so without the overrides a DTO would oblige its crate to declare
//! `serde` / `validator` / `schemars` — the three lines this decorator exists
//! to absorb. It lives in the kernel rather than in HTTP because a wire type
//! crosses queues, gateways and tools too, and none of those should drag in the
//! HTTP stack to get a serde derive.
//!
//! `JsonSchema` is included because it is not optional in practice: `#[routes]`
//! documents every `Json<T>` / `Query<T>` argument in the OpenAPI document, so a
//! DTO without it fails to compile with a trait-bound error pointing at
//! `schema_of` rather than at the missing derive.
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

    // Routed through the surface crate, with each derive's `crate = ` override
    // set to the same path: a derive expands against the *call site's* prelude,
    // so without the override it would still emit `::serde::` internally and
    // oblige the developer to declare a crate `#[input]` exists to absorb.
    quote! {
        #[derive(
            ::nest_rs_core::serde::Serialize,
            ::nest_rs_core::serde::Deserialize,
            ::nest_rs_core::validator::Validate,
            ::nest_rs_core::schemars::JsonSchema,
        )]
        #[serde(crate = "::nest_rs_core::serde", deny_unknown_fields)]
        #[validate(crate = ::nest_rs_core::validator)]
        #[schemars(crate = "::nest_rs_core::schemars")]
        #item
    }
    .into()
}
