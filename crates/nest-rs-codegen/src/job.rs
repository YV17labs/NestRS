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
//! `JobContext` seam. The family's *other* keys are another matter — a key one
//! member takes is answered at every member, and where a member cannot take it
//! the refusal names the fact ([`job_argument_refused`]). Those four are the
//! whole family, and a fifth job decorator joins them by calling in here.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ExprLit, Lit};

use crate::replicas::REPLICAS;
use crate::ungrouped::ungrouped_expr;

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
    match ungrouped_expr(expr) {
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

/// Every cell of the worker-job key table a member refuses, with the fact that
/// makes the key meaningless there — `framework.md`, *The impl half*.
///
/// The table is closed: each of the family's keys beside `transactional`, which
/// every member builds, is built at a member or listed here for it, and the test
/// below holds the two to that. A key no member takes
/// is not a cell, and keeps the unknown-key sentence.
const REFUSED: [(&str, &str, &str); 17] = [
    (
        "process",
        REPLICAS,
        "a job is delivered to one worker, so there is no replica to choose",
    ),
    (
        "process",
        "tz",
        "a job runs when delivered, and has no clock to be read in",
    ),
    ("every", "queue", RUNS_IN_PROCESS),
    ("every", "retries", "a tick's retry is the next occurrence"),
    ("every", "concurrency", NEVER_OVERLAPS),
    ("every", "throttle", "the trigger is the rate"),
    ("every", "tz", "an interval has no wall clock"),
    ("cron", "queue", RUNS_IN_PROCESS),
    ("cron", "retries", "a tick's retry is the next occurrence"),
    ("cron", "concurrency", NEVER_OVERLAPS),
    ("cron", "throttle", "the trigger is the rate"),
    ("after", "queue", RUNS_IN_PROCESS),
    (
        "after",
        "retries",
        "a one-shot has no next occurrence, so work that must succeed is a queue job it pushes",
    ),
    ("after", "concurrency", "a one-shot runs once"),
    ("after", "throttle", "the trigger is the rate"),
    (
        "after",
        REPLICAS,
        "a one-shot fires on the replica that booted",
    ),
    ("after", "tz", "a delay has no wall clock"),
];

/// The one delivery fact all three triggers state: `queue` names where a
/// `#[process]` job is delivered *from*, and a tick is delivered from nowhere.
const RUNS_IN_PROCESS: &str = "a scheduled tick runs in process and is not delivered from a \
     queue — push a job from the tick to reach one";

/// The one overlap fact both recurring triggers state.
const NEVER_OVERLAPS: &str = "a scheduled job never overlaps itself — an occurrence falling \
     inside a run is skipped and counted";

/// The refusal of `key` at `#[attr]` when another member of the worker-job family
/// takes it and this one cannot, naming why — `None` for a key this member takes,
/// and for a key no member takes, which [`crate::unknown_argument`] answers.
///
/// Checked before the unknown-key sentence and whatever the value, so a
/// developer carrying `retries` from a `#[process]` to an `#[every]` learns what
/// a tick does instead, not that they misspelled a word.
pub fn job_argument_refused(attr: &str, key: &str) -> Option<String> {
    REFUSED
        .iter()
        .find(|(member, refused, _)| *member == attr && *refused == key)
        .map(|(_, _, fact)| format!("#[{attr}] takes no `{key}`: {fact}"))
}

/// The refusal of a job method answering `()` — written or not — naming what its
/// `Result` decides. `#[process]` and the three triggers state it one way; only
/// what an `Err` does differs, because a job has a retry budget and an occurrence
/// has the next one.
pub fn job_returns_a_result(attr: &str) -> String {
    let outcome = if attr == "process" {
        "`Ok(())` completes the job and an `Err` fails the attempt, which the retry budget \
         decides on"
    } else {
        "`Ok(())` completes the occurrence and an `Err` fails it, which the scheduler logs \
         before the next one"
    };
    format!("a `#[{attr}]` method returns a `Result`: {outcome} — write `-> anyhow::Result<()>`")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every member answers every family key: built, or refused with a fact.
    #[test]
    fn the_key_table_is_closed() {
        const FAMILY_KEYS: [&str; 6] = [
            "queue",
            "retries",
            "concurrency",
            "throttle",
            REPLICAS,
            "tz",
        ];
        let built: [(&str, &[&str]); 4] = [
            ("process", &["queue", "retries", "concurrency", "throttle"]),
            ("every", &[REPLICAS]),
            ("cron", &[REPLICAS, "tz"]),
            ("after", &[]),
        ];
        for (member, takes) in built {
            for key in FAMILY_KEYS {
                assert_ne!(
                    takes.contains(&key),
                    job_argument_refused(member, key).is_some(),
                    "#[{member}] `{key}` is neither built nor refused, or both",
                );
            }
        }
        assert_eq!(job_argument_refused("every", "priority"), None);
    }
}
