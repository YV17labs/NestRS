//! The `replicas` key — whether an occurrence of a scheduled job fires on every
//! replica of the app, or on exactly one.
//!
//! `#[every]` and `#[cron]` take it after their trigger, beside `transactional`.
//! `#[after]` and `#[process]` refuse it, each naming why, through the family's
//! one table — `job::job_key`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ExprLit, Lit};

use crate::args::{takes_one_of, unknown_value};
use crate::job::JobDecorator;
use crate::ungrouped::ungrouped_expr;

/// The key, spelled once.
pub const REPLICAS: &str = "replicas";

/// The values, in the order a refusal lists them.
const VALUES: [&str; 2] = ["each", "one"];

/// What each value does — the choice a developer reaching for this key is making,
/// and so the half of every refusal worth reading.
const WHAT_THE_VALUES_DO: &str = "`\"each\"` (the default) fires every occurrence on every replica \
     of the app; `\"one\"` fires each occurrence on exactly one replica, the one that claims it \
     through the occurrence lock the app imports";

/// The `Replicas` variant a `replicas = …` value written at `#[member]` selects,
/// rooted at the surface crate the calling macro emits through
/// (`::nest_rs_schedule`).
///
/// Both refusals name the decorator: a value of the wrong kind through
/// [`crate::args::takes_value`], listing [`VALUES`] as the string literals the
/// key takes, and a string outside them through [`unknown_value`].
pub fn replicas_value(
    member: JobDecorator,
    expr: &Expr,
    surface: &TokenStream,
) -> syn::Result<TokenStream> {
    let attr = member.name();
    let unwrapped = ungrouped_expr(expr);
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = unwrapped
    else {
        let literals = VALUES.map(|value| format!("{value:?}"));
        return Err(syn::Error::new_spanned(
            unwrapped,
            format!(
                "{} — {WHAT_THE_VALUES_DO}",
                takes_one_of(attr, REPLICAS, &literals.each_ref().map(String::as_str))
            ),
        ));
    };
    match value.value().as_str() {
        "each" => Ok(quote! { #surface::Replicas::Each }),
        "one" => Ok(quote! { #surface::Replicas::One }),
        other => Err(syn::Error::new_spanned(
            value,
            format!(
                "{} — {WHAT_THE_VALUES_DO}",
                unknown_value(attr, REPLICAS, other, &VALUES)
            ),
        )),
    }
}

/// The variant an unwritten key selects — every replica, which is what a
/// schedule did before the key existed, spelled out so the expansion states it.
pub fn replicas_default(surface: &TokenStream) -> TokenStream {
    quote! { #surface::Replicas::Each }
}
