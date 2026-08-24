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

/// What each value *does* — the half of both refusals below that a developer is
/// actually choosing between, and the reason both are worded here: one key, four
/// decorators, one sentence.
const WHAT_THE_VALUES_DO: &str = "`true` (the default) settles the job's data-layer work as one \
     transaction per attempt, so a failed attempt leaves nothing for the retry to repeat; `false` \
     runs it on the pool, for a job that brackets long work that is not the database's";

/// Read a `transactional = …` value off the expression the key was given.
///
/// The sentence names what each value *does* rather than the type it wanted: a
/// developer reaching for this key is choosing between two behaviours, not
/// fixing a typo, and the choice is the thing worth stating at the point of
/// refusal.
pub fn transactional_value(expr: &Expr) -> syn::Result<bool> {
    match unwrap_groups(expr) {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(b), ..
        }) => Ok(b.value()),
        other => Err(syn::Error::new_spanned(
            other,
            format!("`{TRANSACTIONAL}` takes `true` or `false` — {WHAT_THE_VALUES_DO}"),
        )),
    }
}

/// The refusal for the key written **bare**, with no value.
///
/// A separate sentence from the one above, because the two mistakes differ: a
/// wrong value is a choice mis-typed, a missing one is a declaration that says
/// nothing. What both sites owed and neither gave was the second half — a bare
/// `expected =` names the grammar and not the key, and on a trigger it did not
/// even land on the right decorator, the argument list parsing through a
/// `Punctuated` whose failure is reported at the enclosing `#[scheduled]`.
fn transactional_needs_a_value(attr: &str) -> String {
    format!(
        "#[{attr}] `{TRANSACTIONAL}` needs a value — write `{TRANSACTIONAL} = true` or \
         `{TRANSACTIONAL} = false`. {WHAT_THE_VALUES_DO}"
    )
}

/// The refusal for **any** key of a job decorator written bare — the shared
/// sentence, or the `transactional` one when that is the key.
///
/// One call rather than the `if name == TRANSACTIONAL { … } else { … }` both job
/// decorators had written out: the branch is the shared thing, not just its two
/// arms.
pub fn job_argument_needs_a_value(attr: &str, name: &str) -> String {
    if name == TRANSACTIONAL {
        transactional_needs_a_value(attr)
    } else {
        crate::args::needs_a_value(attr, name)
    }
}

/// Strip the invisible-delimiter groups a `macro_rules!` substitution leaves.
///
/// A `$settle:expr` reaches a proc macro as `Expr::Group` — `syn` unwraps it in
/// some parse paths and not others, and *which* depended here on how each
/// decorator read its argument list and on where in that list the key sat.
/// `#[process]` parses the value with `input.parse::<Expr>()` and keeps the
/// group; the triggers go through `Punctuated<MetaNameValue, …>`, which unwraps
/// only when the fork is empty — so the same `false` compiled as the *last*
/// argument of `#[every]` and was refused one position earlier in `#[cron]`,
/// with a sentence telling the developer to write the value they had written.
/// Unwrapping here is what makes "one key, four sites, one answer" true rather
/// than nearly true. Looping, not a single unwrap: nesting is legal, and a key
/// forwarded through two macro layers arrives wrapped twice.
fn unwrap_groups(expr: &Expr) -> &Expr {
    let mut current = expr;
    while let Expr::Group(group) = current {
        current = &group.expr;
    }
    current
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
