//! Attribute-argument parsing helpers shared by the decorator macros.

use proc_macro2::TokenStream as TokenStream2;
use syn::parse::{ParseStream, Parser};
use syn::{Expr, ExprLit, Ident, Lit, LitStr, Token};

/// Parse a decorator's sole `<key> = "..."` string argument from its attribute
/// tokens — `#[controller(path = "…")]`, `#[mcp(path = "…")]`, etc. `key`
/// is the expected argument name, `attr` the attribute; both appear in the error.
pub fn parse_named_str_arg(args: TokenStream2, key: &str, attr: &str) -> syn::Result<LitStr> {
    let parser = |input: ParseStream| -> syn::Result<LitStr> {
        let found: Ident = input.parse()?;
        if found != key {
            return Err(syn::Error::new(
                found.span(),
                format!("expected `{key} = \"...\"` as the only #[{attr}] argument"),
            ));
        }
        input.parse::<Token![=]>()?;
        input.parse()
    };
    parser.parse2(args)
}

/// Interpret an already-parsed attribute-argument value as a string literal,
/// cloning it out — the value half of a `syn::MetaNameValue` you already hold,
/// as opposed to [`parse_named_str_arg`], which parses the whole `key = "..."`.
/// On a non-string value it errors (spanned at the value) with a message naming
/// the decorator and key — ``#[{attr}] `{key}` must be a string literal, e.g.
/// `{key} = "{example}"` `` — where `example` is the placeholder value shown in
/// the hint (`"database"`, `"..."`).
pub fn require_str_lit(value: &Expr, attr: &str, key: &str, example: &str) -> syn::Result<LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = value
    {
        Ok(s.clone())
    } else {
        Err(syn::Error::new_spanned(
            value,
            format!("#[{attr}] `{key}` must be a string literal, e.g. `{key} = \"{example}\"`"),
        ))
    }
}

/// The sentence a decorator prints when one of its arguments is written twice.
///
/// Accepting the repeat means dropping one of two declarations, and which one
/// it drops is source order — the shape every unified grammar here exists to
/// remove. So the refusal is worded once, for every decorator whose arguments
/// are a list of `key = value` pairs, rather than per key: a refusal that
/// multiplies with the argument matrix is a refusal that gets skipped.
///
/// The caller supplies the span, because the two shapes that need this parse
/// their arguments differently — an `Ident` in a hand-rolled loop, a
/// `MetaNameValue`'s path in a `Punctuated` — and the span is what puts the
/// error under the *second* spelling rather than the whole attribute.
pub fn duplicate_argument(attr: &str, name: &str) -> String {
    format!("#[{attr}] takes at most one `{name}`")
}

/// The sentence a decorator prints for an argument written **bare**, with no
/// value.
///
/// The third of the three refusals a `key = value` grammar owes, beside
/// [`duplicate_argument`] and [`unknown_argument`], and worded here for the same
/// reason: a bare `expected `=`` names the grammar and not the key, and two
/// sites wording it themselves is how one of them ends up with its decorator
/// name as a literal.
///
/// A key that has more to say about *which* values it takes wraps this — see
/// `job::transactional_needs_a_value`.
pub fn needs_a_value(attr: &str, name: &str) -> String {
    format!("#[{attr}] `{name}` needs a value — write `{name} = ...`")
}

/// The sentence a decorator prints for an argument it does not know, listing the
/// ones it does.
///
/// Worded once for the same reason [`duplicate_argument`] is, and it arrived
/// later for a reason worth remembering: the two halves of the job family had
/// drifted into two forms — ``unknown #[process] key `x` (expected …)`` against
/// ``unknown #[every] argument `x`; expected …`` — and the first spelled
/// `transactional` as a literal in a file that already imports the constant. A
/// shared key whose refusal reads differently at two of its four sites is a
/// shared key on paper.
///
/// `expected` is listed in the order the decorator declares it, joined with a
/// final `or`: an alphabetical sort would put the required argument last on
/// half the decorators.
pub fn unknown_argument(attr: &str, name: &str, expected: &[&str]) -> String {
    let quoted: Vec<String> = expected.iter().map(|key| format!("`{key}`")).collect();
    let list = match quoted.split_last() {
        None => "no arguments".to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    };
    format!("unknown #[{attr}] argument `{name}`; expected {list}")
}
