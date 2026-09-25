//! A value without the invisible groups a `macro_rules!` substitution wraps it
//! in — one concern, read at three token shapes.
//!
//! A `$x:expr`, `$t:ty` or `$m:meta` fragment reaches a proc macro wrapped in a
//! `Delimiter::None` group, and `syn` unwraps it in some parse paths and not
//! others. A decorator that matches on the value's shape has to look through the
//! group, or the same literal compiles written by hand and is refused when a
//! macro forwarded it. The three readers are named `ungrouped_<shape>` so the
//! next one — a pattern, a statement — has one obvious name.

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::{Expr, Type};

/// `expr` without the invisible groups around it.
///
/// Looping, not a single unwrap: nesting is legal, and a value forwarded through
/// two macro layers arrives wrapped twice. `#[process]` parses a value with
/// `input.parse::<Expr>()` and keeps the group, while the triggers read theirs
/// through a `Punctuated<Meta, _>` that unwraps only when the fork is empty — so
/// the same `false` compiled as the *last* argument of `#[every]` and was refused
/// one position earlier in `#[cron]`. Reading every value through here is what
/// makes one key one answer.
pub fn ungrouped_expr(expr: &Expr) -> &Expr {
    let mut current = expr;
    while let Expr::Group(group) = current {
        current = &group.expr;
    }
    current
}

/// `ty` without the parentheses or invisible groups around it.
pub(crate) fn ungrouped_type(ty: &Type) -> &Type {
    match ty {
        Type::Group(group) => ungrouped_type(&group.elem),
        Type::Paren(paren) => ungrouped_type(&paren.elem),
        other => other,
    }
}

/// `tokens` out of the invisible group a `$m:meta` fragment passed into a
/// `cfg_attr` arrives wrapped in.
pub(crate) fn ungrouped_tokens(tokens: TokenStream) -> TokenStream {
    let mut trees = tokens.clone().into_iter();
    match (trees.next(), trees.next()) {
        (Some(TokenTree::Group(group)), None) if group.delimiter() == Delimiter::None => {
            ungrouped_tokens(group.stream())
        }
        _ => tokens,
    }
}
