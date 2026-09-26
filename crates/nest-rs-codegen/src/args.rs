//! Attribute-argument parsing helpers shared by the decorator macros.

use quote::ToTokens;
use syn::{Expr, ExprLit, Lit, LitStr, Meta};

use crate::ungrouped::ungrouped_expr;

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
/// On a non-string value it errors (spanned at the value) through
/// [`takes_value`] — ``#[{attr}] `{key}` takes a string literal, e.g.
/// `{key} = "{example}"` `` — where `example` is the placeholder value shown in
/// the hint (`"seaorm"`, `"..."`).
///
/// **A value a `macro_rules!` forwarded is read through its invisible group**,
/// as every other value reader here reads it ([`crate::ungrouped_expr`]). This
/// one did not, so a literal passed down as `$path:expr` was refused as "not a
/// string literal" by the one reader whose whole question is whether it is one —
/// and only where syn had kept the group, which depends on the argument's
/// position in the list.
pub fn require_str_lit(value: &Expr, attr: &str, key: &str, example: &str) -> syn::Result<LitStr> {
    match ungrouped_expr(value) {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            takes_value(
                attr,
                Some(key),
                &format!("a string literal, e.g. `{key} = \"{example}\"`"),
            ),
        )),
    }
}

/// Where a value refusal points: the decorator, then the key in backticks —
/// ``#[process] `retries` `` — or the decorator alone for the one positional
/// argument a trigger takes (`#[every("30s")]`).
///
/// **Every value refusal this crate words opens with it**, and it is worded
/// here so that stays one fact rather than a convention. Two of them did not:
/// `transactional_value` and `replicas_value` refused a value of the wrong kind
/// with ``` `transactional` takes … ```, naming the key and not the decorator,
/// beside `unknown_value` and the duration grammar, which named both. Read
/// without its source frame — a problems list, a CI summary — such a sentence
/// said which key and not whose, and `transactional` is a key of four
/// decorators.
pub(crate) fn site(attr: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("#[{attr}] `{key}`"),
        None => format!("#[{attr}]"),
    }
}

/// The sentence a decorator prints for a **value of the wrong kind** — a number
/// where a string goes, a string where `true` or `false` does, a path where a
/// literal does: ``#[process] `retries` takes a whole number``.
///
/// The sixth refusal a declaration grammar owes, beside [`duplicate_argument`],
/// [`unknown_argument`], [`needs_a_value`], [`unknown_value`] and
/// [`missing_argument`], and worded here for their reason: `#[process]`'s
/// `retries`, `concurrency` and `throttle` values, the shared `transactional`
/// and `replicas` values, the duration grammar and [`require_str_lit`] each
/// wrote it — in two verbs, and five of the eight without the decorator.
/// `what` is what the key takes, as a phrase; a site with more to say — what
/// each value *does* — appends it after ` — `.
///
/// `key` is `None` for a positional argument, which has no key to name.
pub fn takes_value(attr: &str, key: Option<&str>, what: &str) -> String {
    format!("{} takes {what}", site(attr, key))
}

/// [`takes_value`] for a key whose values are a closed set, listed the way
/// every refusal here lists alternatives.
pub(crate) fn takes_one_of(attr: &str, key: &str, values: &[&str]) -> String {
    takes_value(attr, Some(key), &expected_list(values, "no value"))
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
/// alone was not enough: six decorators wrapped it in their own guard, and the
/// four written for this change had already drifted apart — a `&Option<T>`, a
/// `bool`, a closure over a `&Meta`, and an inline field test — so the span the
/// caret lands on was decided six times. `at` is anything that carries tokens,
/// which is the one axis that genuinely differs: the `Ident` a hand-rolled
/// `ParseStream` loop holds, and the `Meta` or path a `Punctuated` one does.
pub fn reject_duplicate_argument<T: ToTokens>(
    taken: bool,
    at: &T,
    attr: &str,
    name: &str,
) -> syn::Result<()> {
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
///
/// **A nested key's remedy goes inside its parentheses**, because that is where
/// the value it is missing is written. A caller names such a key the way the
/// other two refusals do — `throttle(limit)` — and spelling the remedy as
/// `throttle(limit) = ...` prescribes an edit that does not parse. Handled here
/// rather than at the call site so every nested grammar gets it: the sentence is
/// shared, so its remedy is too.
pub fn needs_a_value(attr: &str, name: &str) -> String {
    format!(
        "{} needs a value — write `{}`",
        site(attr, Some(name)),
        with_a_value(name, "..."),
    )
}

/// `name`, with ` = {value}` written where its value belongs: after the key, or
/// inside the innermost parentheses when the key is nested.
///
/// **The innermost, not the first** — `split_once('(')` reads `a(b(c))` as
/// `a` + `b(c))` and renders `a(b(c) = ...)`, which is unbalanced and does not
/// parse. No two-level grammar exists in the repo today, so that was latent
/// rather than live; it is written correctly here because the next one inherits
/// it, and because the whole point of this sentence is that a reader can paste
/// the remedy.
fn with_a_value(name: &str, value: &str) -> String {
    let core = name.trim_end_matches(')');
    let closes = &name[core.len()..];
    match core.rsplit_once('(') {
        Some((parent, key)) => format!("{parent}({key} = {value}{closes}"),
        None => format!("{name} = {value}"),
    }
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

/// The sentence a decorator prints for a **required** argument that was not
/// written at all.
///
/// The fifth refusal a `key = value` grammar owes, and the last one to be
/// worded here. It was live at eight sites in six crates in three verbs
/// (`requires` / `needs` / `is required`) and three shapes (`requires <key>`,
/// `requires a <key> argument`, `<key> is required`) — and six of the eight were
/// spanned at `Span::call_site()`, so the caret landed on the item rather than
/// on the declaration that is short a key. That is the same drift
/// [`unknown_argument`] was extracted to end, one refusal over.
///
/// `example` is a value the key actually takes, because "requires `path`" tells
/// a reader which key and not what to write there; every sibling in this module
/// carries one for the same reason.
pub fn missing_argument(attr: &str, key: &str, example: &str) -> String {
    format!(
        "#[{attr}] requires `{key}` — write `{}`",
        with_a_value(key, example),
    )
}

/// The one role attribute a decorated method carries — its index in `attrs`,
/// `None` when it carries none — or the refusal of a method carrying more.
///
/// All nine impl-half decorators impose this rule — `#[routes]`, `#[messages]`,
/// `#[operations]`, `#[tools]`, `#[processor]`, `#[scheduled]`, `#[listeners]`,
/// `#[indicators]` and `#[hooks]` — and it was worded four ways plus two
/// silences: `#[routes]` and `#[messages]` took the first verb and left the rest
/// on the method, and `#[tools]` took the first role and let rmcp route the
/// second as an operation nobody declared. **The span is chosen here, never by
/// the caller**: the error sits on the *second* role attribute, the one that
/// made the method two, because six callers spanning it themselves had put the
/// caret on the signature, on the `#`, and on the attribute, in no pattern.
///
/// A role is an attribute whose path is one of `accepted`, written bare
/// (`"get"`, `"on_module_init"`) and bracketed here, so `#[..]` is written once
/// rather than at each call site. `noun` is what the family is called at this
/// site — a phase, a probe, a trigger, a role — because that is the word the
/// developer just wrote and the one they will search for. `why` is what a site
/// has to add about its own roles (GraphQL's `_entities` root), empty for the
/// rest.
///
/// The sentence carries both what the method wrote and what is accepted. **One
/// attribute written again is its own sentence**: both copies name the same
/// role, so no role nobody wrote could run, and saying so would be false. A role
/// repeated beside another is named once.
pub fn one_role_per_method(
    noun: &str,
    attrs: &[syn::Attribute],
    accepted: &[&str],
    why: &str,
) -> syn::Result<Option<usize>> {
    let roles: Vec<usize> = attrs
        .iter()
        .enumerate()
        .filter(|(_, attr)| accepted.iter().any(|name| attr.path().is_ident(name)))
        .map(|(index, _)| index)
        .collect();
    let second = match roles.as_slice() {
        [] => return Ok(None),
        [only] => return Ok(Some(*only)),
        [_, second, ..] => *second,
    };
    let declared: Vec<String> = roles
        .iter()
        .map(|index| key_as_written(attrs[*index].path()))
        .collect();
    Err(syn::Error::new_spanned(
        &attrs[second],
        format!("{}{why}", role_sentence(noun, &declared, accepted)),
    ))
}

/// The sentence [`one_role_per_method`] refuses with, over the role names the
/// method wrote — a repeated one included, so a repetition is counted as written.
fn role_sentence(noun: &str, declared: &[String], accepted: &[&str]) -> String {
    let bracketed: Vec<String> = accepted.iter().map(|name| format!("#[{name}]")).collect();
    let accepted = expected_list(
        bracketed
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
        "none",
    );
    let mut distinct: Vec<&str> = Vec::new();
    for name in declared {
        if !distinct.contains(&name.as_str()) {
            distinct.push(name);
        }
    }
    if let [name] = distinct.as_slice() {
        let times = match declared.len() {
            2 => "twice".to_owned(),
            count => format!("{count} times"),
        };
        return format!(
            "a method declares exactly one {noun} — this one writes `#[{name}]` {times}. \
             Accepted: {accepted}. A method that must be two is two methods."
        );
    }
    // `and`, not the `or` [`expected_list`] joins with: the method declared
    // both of these, and reading it back as a choice describes the opposite of
    // what happened.
    let written = distinct
        .iter()
        .map(|name| format!("`#[{name}]`"))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        "a method declares exactly one {noun} — this one declares {written}. Accepted: \
         {accepted}. Keeping the first and dropping the second would run the method under \
         a {noun} you did not write, so neither is assumed: a method that must be two is \
         two methods."
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
/// needs the same reader and cannot use [`unmatched_meta`]. Three shapes, one
/// answer to "what did they actually write": the third is
/// [`one_role_per_method`] naming the *role* attributes a method declared.
///
/// That third caller had its own copy, returning `?` for a non-ident path —
/// the "nicer placeholder" the paragraph above condemns, ninety lines from the
/// sentence condemning it. One fallback, because one of the two has to be
/// right and a reader picking between them at a new call site had no basis.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A literal a `macro_rules!` forwarded as `$x:expr` reaches the reader
    /// inside an invisible group, and is still the literal.
    #[test]
    fn a_forwarded_string_literal_is_read_through_its_group() {
        let forwarded = Expr::Group(syn::ExprGroup {
            attrs: Vec::new(),
            group_token: Default::default(),
            expr: Box::new(syn::parse_quote!("/users")),
        });
        let literal = require_str_lit(&forwarded, "controller", "path", "/users")
            .expect("a string literal, forwarded");
        assert_eq!(literal.value(), "/users");
    }

    fn refusal(method: syn::ImplItemFn, accepted: &[&str]) -> String {
        one_role_per_method("probe", &method.attrs, accepted, "")
            .expect_err("more than one role")
            .to_string()
    }

    /// Every value refusal this crate words opens with its site — the decorator,
    /// then the key — which is the one fact [`site`] exists to keep. Listed by
    /// hand, as the members of a unit test are; the list is every function here
    /// that refuses a value, and a new one joins it the day it is written.
    #[test]
    fn every_value_refusal_opens_with_the_decorator_and_the_key() {
        use quote::quote;
        use syn::parse_quote;

        let err = |result: syn::Result<()>| result.expect_err("a refused value").to_string();
        let refusals = [
            (
                err(require_str_lit(&parse_quote!(42), "probe", "path", "/x").map(drop)),
                "path",
            ),
            (needs_a_value("probe", "path"), "path"),
            (
                err(crate::job::transactional_value("probe", &parse_quote!("no")).map(drop)),
                "transactional",
            ),
            (
                crate::job::job_argument_needs_a_value("probe", "transactional"),
                "transactional",
            ),
            (
                err(
                    crate::replicas::replicas_value("probe", &parse_quote!(one), &quote!(::x))
                        .map(drop),
                ),
                "replicas",
            ),
            (
                err(
                    crate::duration::duration_millis("probe", Some("window"), &parse_quote!(60))
                        .map(drop),
                ),
                "window",
            ),
            (
                crate::queue_name::invalid_queue_name("probe", "name", "a b"),
                "name",
            ),
            (
                err(crate::mount::reject_path(
                    "probe",
                    &parse_quote!("no leading slash"),
                )),
                "path",
            ),
            (
                err(
                    crate::versioning::parse_version_list(&parse_quote!("a b"), "#[probe]")
                        .map(drop),
                ),
                "version",
            ),
            (
                err(
                    crate::versioning::parse_version_list(&parse_quote!(["1", "1"]), "#[probe]")
                        .map(drop),
                ),
                "version",
            ),
            (
                err(crate::versioning::parse_version_list(&parse_quote!([]), "#[probe]").map(drop)),
                "version = []",
            ),
        ];
        for (refusal, key) in refusals {
            assert!(
                refusal.starts_with(&format!("#[probe] `{key}`")),
                "{refusal}"
            );
        }

        // The positional grammar has no key, and names the decorator alone.
        let positional =
            err(crate::duration::duration_millis("probe", None, &parse_quote!(60)).map(drop));
        assert!(positional.starts_with("#[probe] takes "), "{positional}");

        // `#[crud]` words its own, at its own name — a value of the wrong kind
        // included, which its parser used to leave to syn's `expected
        // identifier`.
        for (args, key) in [
            (
                quote!(service = svc, entity = E, output = O, ops = []),
                "ops = []",
            ),
            (
                quote!(service = svc, entity = E, output = O, ops = [create]),
                "ops",
            ),
            (quote!(service = "svc", entity = E, output = O), "service"),
            (quote!(service = svc, entity = 42, output = O), "entity"),
            (quote!(service = svc, entity = E, output = 42), "output"),
            (
                quote!(service = svc, entity = E, output = O, create = "C"),
                "create",
            ),
            (
                quote!(service = svc, entity = E, output = O, update = "U"),
                "update",
            ),
            (
                quote!(service = svc, entity = E, output = O, ops = list),
                "ops",
            ),
            (
                quote!(service = svc, entity = E, output = O, ops = ["list"]),
                "ops",
            ),
            (
                quote!(service = svc, entity = E, output = O, paginate = "cursor"),
                "paginate",
            ),
        ] {
            let refusal = crate::crud::parse_crud_args(args)
                .and_then(|declaration| declaration.generated_ops().map(drop))
                .expect_err("a refused value")
                .to_string();
            assert!(
                refusal.starts_with(&format!("#[crud] `{key}`")),
                "{refusal}"
            );
        }

        // A value outside a closed set keeps the `unknown` shape, which names
        // both as well.
        let unknown =
            err(
                crate::replicas::replicas_value("probe", &parse_quote!("all"), &quote!(::x))
                    .map(drop),
            );
        assert!(
            unknown.starts_with("unknown #[probe] replicas `all`"),
            "{unknown}"
        );
    }

    #[test]
    fn a_nested_keys_remedy_is_written_inside_its_parentheses() {
        assert_eq!(
            needs_a_value("process", "throttle(limit)"),
            "#[process] `throttle(limit)` needs a value — write `throttle(limit = ...)`",
        );
        assert_eq!(
            needs_a_value("process", "retries"),
            "#[process] `retries` needs a value — write `retries = ...`",
        );
    }

    #[test]
    fn a_nested_remedy_holds_at_any_depth() {
        assert_eq!(
            needs_a_value("process", "a(b(c))"),
            "#[process] `a(b(c))` needs a value — write `a(b(c = ...))`",
        );
        assert_eq!(
            missing_argument("process", "throttle(window)", "\"1m\""),
            "#[process] requires `throttle(window)` — write `throttle(window = \"1m\")`",
        );
        assert_eq!(
            missing_argument("controller", "path", "\"/users\""),
            "#[controller] requires `path` — write `path = \"/users\"`",
        );
    }

    #[test]
    fn one_role_is_found_and_none_is_no_role() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[doc = "x"] #[readiness] async fn ready(&self) {}
        };
        assert_eq!(
            one_role_per_method("probe", &method.attrs, &["liveness", "readiness"], "").ok(),
            Some(Some(1))
        );
        assert_eq!(
            one_role_per_method("probe", &method.attrs, &["startup"], "").ok(),
            Some(None)
        );
    }

    #[test]
    fn two_roles_are_named_with_why_neither_is_kept() {
        let sentence = refusal(
            syn::parse_quote! { #[liveness] #[readiness] async fn probe(&self) {} },
            &["liveness", "readiness", "startup"],
        );
        assert!(
            sentence.contains("declares `#[liveness]` and `#[readiness]`."),
            "{sentence}"
        );
        assert!(sentence.contains("a probe you did not write"), "{sentence}");
    }

    #[test]
    fn one_role_written_again_is_said_to_be_repeated_and_nothing_more() {
        let twice = refusal(
            syn::parse_quote! { #[startup] #[startup] async fn probe(&self) {} },
            &["startup"],
        );
        assert!(twice.contains("writes `#[startup]` twice."), "{twice}");
        assert!(!twice.contains("did not write"), "{twice}");
        let thrice = refusal(
            syn::parse_quote! { #[startup] #[startup] #[startup] async fn probe(&self) {} },
            &["startup"],
        );
        assert!(thrice.contains("writes `#[startup]` 3 times."), "{thrice}");
    }

    #[test]
    fn a_role_repeated_beside_another_is_named_once() {
        let sentence = refusal(
            syn::parse_quote! {
                #[on_module_init] #[on_module_init] #[on_module_destroy] async fn hook(&self) {}
            },
            &["on_module_init", "on_module_destroy"],
        );
        assert!(
            sentence.contains("declares `#[on_module_init]` and `#[on_module_destroy]`."),
            "{sentence}"
        );
    }

    #[test]
    fn a_site_with_more_to_say_appends_it() {
        let method: syn::ImplItemFn =
            syn::parse_quote! { #[query] #[entity] async fn find(&self) {} };
        let error = one_role_per_method("role", &method.attrs, &["query", "entity"], " Why.")
            .expect_err("two roles");
        assert!(error.to_string().ends_with("two methods. Why."), "{error}");
    }
}
