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

/// The `replicas` a job declared — read here rather than left as tokens, because
/// a key beside it depends on which: `key` pins what a job firing once claims
/// under, and means nothing on one firing on every replica.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Replicas {
    /// `"each"` — and the key unwritten, which is what a schedule did before the
    /// key existed.
    #[default]
    Each,
    /// `"one"`.
    One,
}

impl Replicas {
    /// The runtime's `Replicas` variant, rooted at the surface crate the calling
    /// macro emits through (`::nest_rs_schedule`) and spelled out, so the
    /// expansion states which behaviour it chose.
    pub fn tokens(self, surface: &TokenStream) -> TokenStream {
        match self {
            Self::Each => quote! { #surface::Replicas::Each },
            Self::One => quote! { #surface::Replicas::One },
        }
    }
}

/// The [`Replicas`] a `replicas = …` value written at `#[member]` selects.
///
/// Both refusals name the decorator: a value of the wrong kind through
/// [`crate::args::takes_value`], listing [`VALUES`] as the string literals the
/// key takes, and a string outside them through [`unknown_value`].
pub fn replicas_value(member: JobDecorator, expr: &Expr) -> syn::Result<Replicas> {
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
        "each" => Ok(Replicas::Each),
        "one" => Ok(Replicas::One),
        other => Err(syn::Error::new_spanned(
            value,
            format!(
                "{} — {WHAT_THE_VALUES_DO}",
                unknown_value(attr, REPLICAS, other, &VALUES)
            ),
        )),
    }
}
