//! Attribute-argument parsing helpers shared by the decorator macros.

use syn::{Expr, ExprLit, Lit, LitStr};

use crate::ungrouped::ungrouped_expr;

/// Interpret an already-parsed attribute-argument value as a string literal,
/// cloning it out, read through any `macro_rules!` invisible group.
///
/// On a non-string value it errors (spanned at the value) through
/// [`takes_value`] — ``#[{attr}] `{key}` takes a string literal, e.g.
/// `{key} = "{example}"` ``.
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
/// ``#[process] `retries` `` — or the decorator alone for a decorator's one
/// positional argument (`#[every("30s")]`, `#[get("/users")]`).
///
/// Every value refusal a decorator prints opens with it: a value that breaks a
/// rule reads `{site}: {value} is not …`.
pub fn site(attr: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("#[{attr}] `{key}`"),
        None => format!("#[{attr}]"),
    }
}

/// The sentence a decorator prints for a **value of the wrong kind** — a number
/// where a string goes, a string where `true` or `false` does, a path where a
/// literal does: ``#[process] `retries` takes a whole number``.
///
/// `what` is what the key takes, as a phrase; a site with more to say appends
/// it after ` — `. `key` is `None` for a positional argument.
pub fn takes_value(attr: &str, key: Option<&str>, what: &str) -> String {
    format!("{} takes {what}", site(attr, key))
}

/// [`takes_value`] for a key whose values are a closed set, listed the way
/// every refusal here lists alternatives.
pub(crate) fn takes_one_of(attr: &str, key: &str, values: &[&str]) -> String {
    takes_value(attr, Some(key), &expected_list(values, "no value"))
}

/// The sentence a decorator prints when one of its arguments is written twice,
/// issued by [`Grammar`](crate::Grammar) for every key it takes.
pub fn duplicate_argument(attr: &str, name: &str) -> String {
    format!("#[{attr}] takes at most one `{name}`")
}

/// The sentence a decorator prints for an argument written **bare**, with no
/// value.
///
/// A nested key, named `throttle(limit)`, gets its remedy inside the
/// parentheses: `throttle(limit = ...)`.
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
/// The innermost: `split_once('(')` would render `a(b(c) = ...)`, unbalanced.
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
/// `expected` is listed in the order the decorator declares it.
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
/// `what` names the position the value sits in — `"op"`, `"scope"`, the key's
/// own name.
pub fn unknown_value(attr: &str, what: &str, name: &str, expected: &[&str]) -> String {
    format!(
        "unknown #[{attr}] {what} `{name}`; expected {}",
        expected_list(expected, "no value"),
    )
}

/// The sentence a decorator prints for a **required** argument that was not
/// written at all.
///
/// `example` is a value the key actually takes.
pub fn missing_argument(attr: &str, key: &str, example: &str) -> String {
    format!(
        "#[{attr}] requires `{key}` — write `{}`",
        with_a_value(key, example),
    )
}

/// The one role attribute a decorated method carries — its index in `attrs`,
/// `None` when it carries none — or the refusal of a method carrying more.
///
/// The error is spanned at the *second* role attribute. `accepted` names are
/// written bare (`"get"`); `noun` is what the family is called at this site
/// (a phase, a probe, a trigger); `why` is appended, empty for most sites.
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

/// The offending key as written, `::`-joined when it is not a bare identifier.
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

    /// Lists every function here that refuses a value; a new one joins it.
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
                    crate::mount::MountPath::Literal,
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

        let every = crate::job::JobDecorator::Every;
        for (refusal, key) in [
            (
                err(crate::job::transactional_value(every, &parse_quote!("no")).map(drop)),
                "transactional",
            ),
            (
                crate::job::job_argument_needs_a_value(every, "transactional"),
                "transactional",
            ),
            (
                err(crate::replicas::replicas_value(every, &parse_quote!(one)).map(drop)),
                "replicas",
            ),
            (
                err(crate::identity::key_value(every, &parse_quote!(billing)).map(drop)),
                "key",
            ),
            (
                err(crate::identity::key_value(every, &parse_quote!("billing:close")).map(drop)),
                "key",
            ),
            (crate::identity::key_without_replicas_one(every), "key"),
        ] {
            assert!(
                refusal.starts_with(&format!("#[every] `{key}`")),
                "{refusal}"
            );
        }

        let positional =
            err(crate::duration::duration_millis("probe", None, &parse_quote!(60)).map(drop));
        assert!(positional.starts_with("#[probe] takes "), "{positional}");
        for (refusal, site) in [
            (
                err(crate::versioning::parse_version_args(&parse_quote!(#[version(2)])).map(drop)),
                "#[version] takes ",
            ),
            (
                err(
                    crate::versioning::parse_version_args(&parse_quote!(#[version("a b")]))
                        .map(drop),
                ),
                "#[version]: ",
            ),
            (
                err(crate::versioning::parse_version_args(&parse_quote!(#[version()])).map(drop)),
                "#[version] declares ",
            ),
            (
                err(crate::attrs::take_path_list(
                    &mut vec![parse_quote!(#[use_guards("SessionGuard")])],
                    "use_guards",
                )
                .map(drop)),
                "#[use_guards] takes ",
            ),
        ] {
            assert!(refusal.starts_with(site), "{refusal}");
        }

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

        let unknown = err(crate::replicas::replicas_value(every, &parse_quote!("all")).map(drop));
        assert!(
            unknown.starts_with("unknown #[every] replicas `all`"),
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
