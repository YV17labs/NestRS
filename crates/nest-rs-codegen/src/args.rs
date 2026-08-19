//! Attribute-argument parsing helpers shared by the decorator macros.

use quote::ToTokens;
use syn::{Expr, ExprLit, Lit, LitStr, Meta};

/// Interpret an already-parsed attribute-argument value as a string literal,
/// cloning it out — the value half of a `syn::MetaNameValue` you already hold,
/// the caller having parsed the `key =` itself.
///
/// **One sentence for this question, and there were two.** `attrs::expr_str`
/// answered the same one with `"expected a string literal"` at seven call
/// sites — naming neither the decorator nor the key — while this named both,
/// and it lived in the module whose own doc says it handles *whole* attributes
/// "as opposed to [`crate::args`], which parses the values *inside* one". The
/// sharpest instance was `versioning::parse_version_list`, which threads a
/// `decorator` through every refusal it words itself and delegated this one,
/// so `#[controller(version = 1)]` answered with no decorator named inside a
/// function whose whole design is that the sentence names one.
///
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

/// Refuse an argument written twice, spanned at the second spelling.
///
/// The guard half of [`duplicate_argument`], worded here because the sentence
/// alone was not enough: six decorators wrapped it in their own `once`, and the
/// four written for this change had already drifted apart — a `&Option<T>`, a
/// `bool`, a closure over a `&Meta`, and an inline field test — so the span the
/// caret lands on was decided six times. `at` is anything that carries tokens,
/// which is the one axis that genuinely differs: the `Ident` a hand-rolled
/// `ParseStream` loop holds, and the `Meta` or path a `Punctuated` one does.
pub fn once<T: ToTokens>(taken: bool, at: &T, attr: &str, name: &str) -> syn::Result<()> {
    if taken {
        return Err(syn::Error::new_spanned(at, duplicate_argument(attr, name)));
    }
    Ok(())
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
/// `expected` is listed in the order the decorator declares it — see
/// [`expected_list`].
pub fn unknown_argument(attr: &str, name: &str, expected: &[&str]) -> String {
    format!(
        "unknown #[{attr}] argument `{name}`; expected {}",
        expected_list(expected, "no arguments"),
    )
}

/// The sentence a decorator prints for a **value** outside a closed vocabulary —
/// `#[crud(ops = [...])]`'s op names, `#[injectable(scope = …)]`'s scopes,
/// `#[expose]`'s modes.
///
/// The fourth refusal a declaration grammar owes, and the one that had drifted
/// furthest: a key's value set is as much a closed vocabulary as its key set, so
/// naming the offender and listing the alternatives is the same obligation. It
/// was written five ways, three of which named neither the decorator nor the
/// key — a bare `expected `cursor` or `none`` leaves a reader who wrote
/// `#[crud(paginate = pages)]` to guess which of the seven keys the compiler is
/// talking about.
///
/// `what` names the position the value sits in — `"op"`, `"scope"`, the key's
/// own name — because that is what tells the reader *where* in the attribute to
/// look, and a value has no `key =` of its own to point at.
pub fn unknown_value(attr: &str, what: &str, name: &str, expected: &[&str]) -> String {
    format!(
        "unknown #[{attr}] {what} `{name}`; expected {}",
        expected_list(expected, "no value"),
    )
}

/// The refusal a `Punctuated<Meta, _>` grammar owes for a `Meta` none of its
/// arms matched.
///
/// **A bare key is a `Meta::Path`.** Every accepting arm in that shape is
/// guarded `Meta::NameValue(nv) if nv.path.is_ident(…)`, so `#[controller(path)]`
/// falls through to the unknown-key arm — and adopting [`unknown_argument`]
/// there printed *"unknown #[controller] argument `path`; expected `path` or
/// `version`"*, a sentence that declares the key unknown and then lists it. A
/// wrong key name is worse than the bare `expected \`=\`` it replaced, so the
/// two questions are answered here, together, once:
///
/// - the key is one this decorator takes ⇒ [`needs_a_value`];
/// - it is not ⇒ [`unknown_argument`], naming it as written.
///
/// [`key_as_written`] is the input [`unknown_argument`] left to drift after
/// unifying its sentence, and it is now the only spelling of it. This paragraph
/// claimed three copies retired while four survived — one of them in this
/// crate's own `inject.rs`, four modules from the export — and three of the four
/// defaulted to `"<path>"`, which is the same defect `scheduled.rs` records one
/// function above the copy it kept: "a sentence that refuses without saying what
/// it refused", with a nicer placeholder.
pub fn unmatched_meta(attr: &str, meta: &Meta, expected: &[&str]) -> syn::Error {
    let name = key_as_written(meta.path());
    let message = if matches!(meta, Meta::Path(_)) && expected.contains(&name.as_str()) {
        needs_a_value(attr, &name)
    } else {
        unknown_argument(attr, &name, expected)
    };
    syn::Error::new_spanned(meta, message)
}

/// The offending key as written, so a refusal names it rather than only listing
/// the alternatives. A path that is not a bare identifier is reported as
/// written, which is still more than "unknown option" said.
///
/// Public because the other attribute shape — `syn::meta::parser` /
/// `parse_nested_meta`, which hands a `ParseNestedMeta` rather than a `Meta` —
/// needs the same reader and cannot use [`unmatched_meta`]. Two shapes, one
/// answer to "what did they actually write".
pub fn key_as_written(path: &syn::Path) -> String {
    path.get_ident()
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            path.segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        })
}

/// `` `a`, `b` or `c` ``, in the order the decorator declares them.
///
/// Declaration order rather than alphabetical: an alphabetical sort would put
/// the required argument last on half the decorators.
fn expected_list(expected: &[&str], empty: &str) -> String {
    let quoted: Vec<String> = expected.iter().map(|key| format!("`{key}`")).collect();
    match quoted.split_last() {
        None => empty.to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}
