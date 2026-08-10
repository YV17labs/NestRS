//! The compile-time bound an impl-half decorator emits for every guard declared
//! at its site.
//!
//! `Guard::check_graphql` / `check_ws_message` / `check_mcp` all default to
//! `Ok(())`. Correct as "this guard does not apply to that transport", and also
//! the reason `#[use_guards(ThrottlerGuard)]` beside a `#[query]` used to compile,
//! read as a protection, and throttle nothing.
//!
//! A proc macro cannot ask whether a path implements a method — it has no type
//! information. But it does not need to: Rust can be *asked* to prove it. Each
//! transport has a marker trait in `nest-rs-guards`, and the decorator emits one
//! zero-sized assertion per declared guard, which fails at the `#[use_guards]`
//! line with the marker's `#[diagnostic::on_unimplemented]` note.
//!
//! HTTP takes no assertion, and no marker either: `check_http` is the trait's
//! base entry — the one method not behind a feature — so every `Guard` has it and
//! a bound there could never fail. Emitting one would cost a `const` block per
//! guard per route to prove something already true.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::Path;
use syn::spanned::Spanned;

/// Assert every guard in `guards` implements `marker`.
///
/// `marker` is the tokens of the marker trait's absolute path
/// (`::nest_rs_guards::GraphqlGuard`). Emits nothing for an empty list, so a site
/// that declares no guards pays nothing.
///
/// The list arrives concatenated across every operation in the block, so one
/// guard bound beside five methods appears five times. Deduped here rather than
/// at each call site — by rendered path, the same key
/// [`layer_deps`](crate::layer_deps) uses on this very list a line later — so a
/// reused guard costs one assertion, not one per binding.
pub fn guard_capability_bounds<'a>(
    guards: impl IntoIterator<Item = &'a Path>,
    marker: TokenStream,
) -> TokenStream {
    let mut seen = HashSet::new();
    let asserts = guards
        .into_iter()
        .filter(|guard| seen.insert(quote!(#guard).to_string()))
        .map(|guard| {
            // Spanned at the guard's own path so the error underlines the name
            // inside `#[use_guards(...)]` rather than the whole item.
            quote::quote_spanned! { guard.span() =>
                const _: () = {
                    fn __nestrs_assert_guard_capability<T: #marker + ?::core::marker::Sized>() {}
                    let _ = __nestrs_assert_guard_capability::<#guard>;
                };
            }
        })
        .collect::<Vec<_>>();
    quote! { #(#asserts)* }
}
