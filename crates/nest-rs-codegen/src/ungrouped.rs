//! A value without the invisible groups a `macro_rules!` substitution wraps it
//! in: `syn` unwraps a `Delimiter::None` group in some parse paths and not
//! others, so a decorator matching on a value's shape looks through it here.

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::{Expr, Type};

/// `expr` without the invisible groups around it — looping, since a value
/// forwarded through two macro layers arrives wrapped twice.
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
