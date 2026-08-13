//! The `transactional` key — one spelling, one sentence, every decorator that
//! declares a job.
//!
//! `#[process]`, `#[every]`, `#[cron]` and `#[after]` all declare a unit of work
//! a worker transport drives, and all four take the same key to say how its
//! data-layer work is settled. Learning it once is learning it everywhere, and a
//! bad value reads the same wherever it is typed — which is the whole reason the
//! grammar is worded here rather than four times.
//!
//! There is no site that *cannot* take it: every worker job runs through the one
//! `JobContext` seam. So this module carries no refusal — those four are the
//! whole family, and a fifth job decorator joins them by calling in here.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ExprLit, Lit};

/// The key, spelled once.
pub const TRANSACTIONAL: &str = "transactional";

/// Read a `transactional = …` value off the expression the key was given.
///
/// The sentence names what each value *does* rather than the type it wanted: a
/// developer reaching for this key is choosing between two behaviours, not
/// fixing a typo, and the choice is the thing worth stating at the point of
/// refusal.
pub fn transactional_value(expr: &Expr) -> syn::Result<bool> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(b), ..
        }) => Ok(b.value()),
        other => Err(syn::Error::new_spanned(
            other,
            "`transactional` takes `true` or `false` — `true` (the default) settles the job's \
             data-layer work as one transaction per attempt, so a failed attempt leaves nothing \
             for the retry to repeat; `false` runs it on the pool, for a job that brackets long \
             work that is not the database's",
        )),
    }
}

/// The `JobTransaction` variant a parsed value selects, rooted at the surface
/// crate the calling macro emits through (`::nest_rs_queue`, `::nest_rs_schedule`).
///
/// `None` — the key was not written — is `PerAttempt`, spelled out rather than
/// left to `Default` so the expansion states which behaviour it chose.
pub fn job_transaction(value: Option<bool>, surface: &TokenStream) -> TokenStream {
    match value {
        Some(false) => quote! { #surface::nest_rs_worker::JobTransaction::Pool },
        Some(true) | None => quote! { #surface::nest_rs_worker::JobTransaction::PerAttempt },
    }
}
