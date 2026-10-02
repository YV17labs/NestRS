//! The `key` key — the identity a job firing once across replicas claims its
//! occurrences under, pinned rather than derived.
//!
//! A job's identity is its crate, its host struct and its method, so a rename of
//! any of the three starts a new job: during the rolling deploy that ships it, the
//! old replicas and the new each claim and fire every occurrence. `key = "…"` pins
//! the identity the job had, written as the boot line spells it
//! (`key = "billing::InvoiceTasks::close_day"`), and a free path is a key too.
//!
//! `#[every]` and `#[cron]` take it beside `replicas = "one"`, and refuse it
//! beside a job firing on every replica, which claims nothing; `#[after]` and
//! `#[process]` refuse it through the family's one table (`job::job_key`).
//!
//! **Written twice and pinned once.** A job attached by hand reaches the boot
//! with a plain string, so `nest-rs-schedule` checks the same rule there, and its
//! suite runs both copies over one corpus and holds the two refusals to one
//! fact.

use syn::{Expr, ExprLit, Lit, LitStr};

use crate::args::{site, takes_value};
use crate::job::JobDecorator;
use crate::ungrouped::ungrouped_expr;

/// The key, spelled once.
pub const KEY: &str = "key";

/// The separator between a key's levels — the one a Rust path is written with,
/// so a derived identity is pinned by copying it.
const SEPARATOR: &str = "::";

/// What a key is, and why — the fact both refusals state.
const RULE: &str = "it takes one or more levels joined by `::`, each non-empty and free of `:`, \
     whitespace and control characters — `:` separates the levels of the key a backend claims \
     under, and whitespace would reach a log field";

/// Why a job firing on every replica cannot carry a key, and what to do — the
/// fact both refusals state.
const NEEDS_ONE: &str = "a key pins what a job firing once claims its occurrences under, and this \
     job fires on every replica and claims none — declare `replicas = \"one\"` beside it, or \
     remove the key";

/// Whether `value` is a key: one or more levels joined by `::`, each non-empty
/// and free of `:`, whitespace and control characters.
pub fn is_valid_job_key(value: &str) -> bool {
    value.split(SEPARATOR).all(|level| {
        !level.is_empty()
            && !level
                .chars()
                .any(|c| c == ':' || c.is_whitespace() || c.is_control())
    })
}

/// The refusal of a `key` literal outside the rule — the runtime's sentence with
/// the site in front, so the compile error and the boot error read as one.
pub fn invalid_job_key(attr: &str, value: &str) -> String {
    format!(
        "{}: {value:?} is not a job key: {RULE}",
        site(attr, Some(KEY))
    )
}

/// Read a `key = …` value written at `#[member]`: a string literal holding a
/// key, refused naming the decorator otherwise.
pub fn key_value(member: JobDecorator, expr: &Expr) -> syn::Result<LitStr> {
    let unwrapped = ungrouped_expr(expr);
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = unwrapped
    else {
        return Err(syn::Error::new_spanned(
            unwrapped,
            takes_value(
                member.name(),
                Some(KEY),
                "the job's identity as a string — the path its boot line names, e.g. \
                 `key = \"billing::InvoiceTasks::close_day\"`",
            ),
        ));
    };
    if !is_valid_job_key(&value.value()) {
        return Err(syn::Error::new_spanned(
            value,
            invalid_job_key(member.name(), &value.value()),
        ));
    }
    Ok(value.clone())
}

/// The refusal of `key` on a job that does not declare `replicas = "one"` — the
/// boot's sentence for a job attached by hand, with the site in front.
pub fn key_without_replicas_one(member: JobDecorator) -> String {
    format!("{}: {NEEDS_ONE}", site(member.name(), Some(KEY)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_levels_joined_by_two_colons_each_without_a_colon_or_a_space() {
        for valid in [
            "billing::InvoiceTasks::close_day",
            "nightly_close",
            "billing.nightly-close",
            "café::Tâches::purger",
            "r#type",
        ] {
            assert!(is_valid_job_key(valid), "{valid:?}");
        }
        for refused in [
            "",
            "billing::",
            "::billing",
            "billing:::close",
            "billing:close",
            "billing::close day",
            "billing::\tclose",
            "billing::\u{7}",
        ] {
            assert!(!is_valid_job_key(refused), "{refused:?}");
        }
    }
}
