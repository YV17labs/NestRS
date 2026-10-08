//! The `replicas` key — whether an occurrence of a scheduled job fires on every
//! replica of the app, or on exactly one.

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

/// What each value does, stated by every refusal.
const WHAT_THE_VALUES_DO: &str = "`\"each\"` (the default) fires every occurrence on every replica \
     of the app; `\"one\"` fires each occurrence on exactly one replica, the one that claims it \
     through the occurrence lock the app imports";

/// The `replicas` a job declared; `key` is valid only beside [`Replicas::One`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Replicas {
    /// `"each"`, and the key unwritten.
    #[default]
    Each,
    /// `"one"`.
    One,
}

impl Replicas {
    /// The runtime's `Replicas` variant, rooted at `surface` (`::nest_rs_schedule`).
    pub fn tokens(self, surface: &TokenStream) -> TokenStream {
        match self {
            Self::Each => quote! { #surface::Replicas::Each },
            Self::One => quote! { #surface::Replicas::One },
        }
    }
}

/// The [`Replicas`] a `replicas = …` value written at `#[member]` selects.
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
