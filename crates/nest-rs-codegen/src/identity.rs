//! The `key` key — the identity a job firing once across replicas claims its
//! occurrences under, pinned so a rename does not start a new job mid-deploy.
//!
//! `nest-rs-schedule` holds a runtime copy of this rule, pinned against this
//! one by its suite.

use syn::{Expr, ExprLit, Lit, LitStr};

use crate::args::{site, takes_value};
use crate::job::JobDecorator;
use crate::ungrouped::ungrouped_expr;

/// The key, spelled once.
pub const KEY: &str = "key";

/// The separator between a key's levels, as a Rust path writes it.
const SEPARATOR: &str = "::";

const RULE: &str = "it takes one or more levels joined by `::`, each non-empty and free of `:`, \
     whitespace and control characters — `:` separates the levels of the key a backend claims \
     under, and whitespace would reach a log field";

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
/// the site in front.
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

/// The refusal of `key` on a job that does not declare `replicas = "one"`.
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
