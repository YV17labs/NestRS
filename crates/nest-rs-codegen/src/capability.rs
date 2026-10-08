//! The compile-time bound an impl-half decorator emits for every guard declared
//! at its site.
//!
//! Every `Guard::check_*` defaults to `Ok(())`, so a guard bound at an edge it
//! does not check would compile and pass every request. Each edge's marker
//! trait in `nest-rs-guards` (HTTP included) is asserted per declared guard.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};

use crate::attrs::Conditional;
use syn::Path;
use syn::spanned::Spanned;

/// Assert every guard in `guards` implements `marker`.
///
/// `marker` is the tokens of the marker trait's absolute path
/// (`::nest_rs_guards::GraphqlGuard`). Deduped by rendered path, as
/// [`layer_deps`](crate::layer_deps) keys this list.
///
/// A guard bound on a `#[cfg]`-gated method is asserted under its
/// [`Conditional`] conditions — it may not exist in that build — unless it is
/// also bound unconditionally.
pub fn guard_capability_bounds<'a>(
    guards: impl IntoIterator<Item = impl Into<Conditional<'a, Path>>>,
    marker: TokenStream,
) -> TokenStream {
    let guards: Vec<Conditional<'a, Path>> = guards.into_iter().map(Into::into).collect();
    let unconditional: HashSet<String> = guards
        .iter()
        .filter(|entry| entry.cfgs.is_empty())
        .map(|entry| entry.item.to_token_stream().to_string())
        .collect();
    let mut seen = HashSet::new();
    let asserts = guards
        .into_iter()
        .filter(|Conditional { cfgs, item }| {
            let rendered = item.to_token_stream().to_string();
            (cfgs.is_empty() || !unconditional.contains(&rendered))
                && seen.insert(format!("{} {rendered}", quote!(#(#cfgs)*)))
        })
        .map(|Conditional { cfgs, item: guard }| {
            quote::quote_spanned! { guard.span() =>
                #(#cfgs)*
                const _: () = {
                    fn __nestrs_assert_guard_capability<T: #marker + ?::core::marker::Sized>() {}
                    let _ = __nestrs_assert_guard_capability::<#guard>;
                };
            }
        })
        .collect::<Vec<_>>();
    quote! { #(#asserts)* }
}
