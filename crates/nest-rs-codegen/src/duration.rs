//! The duration grammar — `"500ms"`, `"30s"`, `"5m"`, `"1h"` — worded once for
//! every decorator position that takes a span of time.
//!
//! `#[every]` and `#[after]` read their period through it, and so does every
//! queue key that takes a window. Learning the grammar once is learning it
//! everywhere, and a malformed value reads the same wherever it is typed —
//! which is the whole reason it lives here rather than in the first decorator
//! that needed it.
//!
//! **One parser, one sentence, whatever was written.** A value that is not a
//! string literal at all — `#[every(30)]`, `window = SECONDS` — is the same
//! mistake as `"30"`, and it used to get syn's bare "expected string literal"
//! at one site and a hand-worded clause at another, neither naming the
//! decorator. The sentence names the decorator and the position, as every
//! refusal in [`crate::args`] does.

use syn::{Expr, ExprLit, Lit};

use crate::ungrouped::ungrouped_expr;

/// The refusal of anything outside the grammar, naming where it was written:
/// `key` is the position's name (`throttle(window)`), `None` for the one
/// positional argument of a trigger.
fn outside_the_grammar(attr: &str, key: Option<&str>) -> String {
    let site = match key {
        Some(key) => format!("#[{attr}] `{key}`"),
        None => format!("#[{attr}]"),
    };
    format!(
        "{site} takes a duration literal: a whole number above zero with an `ms`, `s`, `m` or \
         `h` suffix, at most `u64::MAX` milliseconds (e.g. \"500ms\", \"30s\", \"5m\", \"1h\")"
    )
}

/// Read a duration written at `#[attr]`'s `key` as whole milliseconds.
///
/// Refused, spanned at the value and with one sentence: anything but a string
/// literal, a literal without one of the four suffixes, a number that is not a
/// whole non-negative integer, zero, and a value too large for `u64`
/// milliseconds. `ms` is matched before `s`, so `"500ms"` is never read as
/// `"500m"` followed by a stray `s`. A value a `macro_rules!` forwarded is read
/// through its invisible group.
pub fn duration_millis(attr: &str, key: Option<&str>, value: &Expr) -> syn::Result<u64> {
    let value = ungrouped_expr(value);
    let bad = || syn::Error::new_spanned(value, outside_the_grammar(attr, key));
    let Expr::Lit(ExprLit {
        lit: Lit::Str(lit), ..
    }) = value
    else {
        return Err(bad());
    };
    let raw = lit.value();
    let s = raw.trim();
    let (number, multiplier) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err(bad());
    };
    // Digits only, checked before the parse: `u64::from_str` also takes a leading
    // `+`, so `"+5s"` read as five seconds although the grammar is a whole number.
    let number = number.trim();
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(bad());
    }
    let value: u64 = number.parse().map_err(|_| bad())?;
    if value == 0 {
        return Err(bad());
    }
    value.checked_mul(multiplier).ok_or_else(bad)
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn read(value: Expr) -> syn::Result<u64> {
        duration_millis("every", None, &value)
    }

    #[test]
    fn each_suffix_scales_to_milliseconds() {
        assert_eq!(read(parse_quote!("500ms")).ok(), Some(500));
        assert_eq!(read(parse_quote!("30s")).ok(), Some(30_000));
        assert_eq!(read(parse_quote!("5m")).ok(), Some(300_000));
        assert_eq!(read(parse_quote!("1h")).ok(), Some(3_600_000));
    }

    #[test]
    fn zero_a_missing_suffix_a_fraction_and_a_non_literal_are_refused_alike() {
        let refusals: Vec<String> = [
            parse_quote!("0s"),
            parse_quote!("10"),
            parse_quote!("1.5s"),
            parse_quote!("-1s"),
            parse_quote!("+5s"),
            parse_quote!("s"),
            parse_quote!("5 minutes"),
            parse_quote!(30),
            parse_quote!(PERIOD),
            parse_quote!("18446744073709551615h"),
        ]
        .into_iter()
        .map(|value| read(value).expect_err("outside the grammar").to_string())
        .collect();
        for refusal in &refusals {
            assert_eq!(refusal, &outside_the_grammar("every", None));
        }
    }

    #[test]
    fn the_sentence_names_the_decorator_and_the_key() {
        let refusal = duration_millis("process", Some("throttle(window)"), &parse_quote!(60))
            .expect_err("not a literal")
            .to_string();
        assert!(
            refusal.starts_with("#[process] `throttle(window)` takes a duration literal"),
            "{refusal}"
        );
    }
}
