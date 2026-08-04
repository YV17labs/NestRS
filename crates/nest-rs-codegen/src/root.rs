//! Where an expansion's framework paths are rooted.
//!
//! A decorator's expansion lands in the *developer's* crate, so every absolute
//! path it emits resolves against **their** extern prelude — which holds only
//! what their manifest declares. Two populations declare different things, and
//! neither can be talked out of it:
//!
//! * **An app** declares the umbrella (`nest-rs`, plus the capability's
//!   feature) and nothing else. Its expansions must be rooted at
//!   `::nest_rs::<concern>`.
//! * **The framework's own crates** — 14 of them use nestrs decorators in their
//!   own `src/` — cannot declare the umbrella: `nest-rs` depends on them, so
//!   that edge is a real cycle Cargo refuses. Their expansions must be rooted
//!   at `::nest_rs_<concern>`.
//!
//! [`reroot`] resolves which of the two the call site is, once, and rewrites
//! the finished token stream. Macros keep emitting the sibling form they always
//! did; the rewrite is the single place the distinction lives, rather than a
//! `crate = ` argument threaded through eleven decorators and ~50 use sites.
//!
//! The walk sees a decorator's **whole** return value, the developer's own item
//! included (`#[input]`/`#[config]` re-emit it, and the response attributes are
//! pass-throughs). A hand-written `::nest_rs_http::X` in such an item is
//! therefore re-rooted too — deliberate: it resolves either way, and the
//! alternative is teaching the walk which tokens came from where.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use proc_macro_crate::{Error, FoundCrate, crate_name};
use proc_macro2::{Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};
use quote::quote;

/// How the call site reaches the umbrella, when it can reach it at all.
enum Umbrella {
    /// The crate under compilation *is* `nest-rs` (its own doctests).
    Itself,
    /// The name the call site declared it under — `nest_rs` unless renamed.
    Named(String),
}

/// Resolved once per compilation unit: `CARGO_MANIFEST_DIR` does not change
/// between macro invocations, and the cold `crate_name` spawns a subprocess.
static UMBRELLA: OnceLock<Option<Umbrella>> = OnceLock::new();

/// Whether each `nest-rs-<concern>` sibling is declared, memoized for the same
/// reason: the answer depends on the manifest, not on the token stream, and
/// `crate_name` stats the filesystem on every call.
static SIBLINGS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

fn umbrella() -> Option<&'static Umbrella> {
    UMBRELLA
        .get_or_init(|| match crate_name("nest-rs") {
            Ok(FoundCrate::Itself) => Some(Umbrella::Itself),
            Ok(FoundCrate::Name(name)) => Some(Umbrella::Named(name)),
            // No umbrella in this manifest: a framework crate, which reaches
            // every concern by its sibling name already.
            Err(_) => None,
        })
        .as_ref()
}

/// `true` when the manifest was readable and holds no such sibling. A build
/// system `proc-macro-crate` cannot introspect answers `false`, so it degrades
/// to the old behaviour instead of failing a build that would have worked.
fn sibling_missing(concern: &str) -> bool {
    let cache = SIBLINGS.get_or_init(Default::default);
    if let Ok(map) = cache.lock()
        && let Some(known) = map.get(concern)
    {
        return *known;
    }
    let name = format!("nest-rs-{}", concern.replace('_', "-"));
    let missing = matches!(crate_name(&name), Err(Error::CrateNotFound { .. }));
    if let Ok(mut map) = cache.lock() {
        map.insert(concern.to_owned(), missing);
    }
    missing
}

fn root_prefix(u: &Umbrella) -> TokenStream {
    match u {
        Umbrella::Itself => quote!(crate),
        Umbrella::Named(name) => {
            let ident = Ident::new(name, Span::call_site());
            quote!(::#ident)
        }
    }
}

/// Rewrite every `::nest_rs_<concern>` root in a finished expansion to the path
/// the call site can actually resolve.
///
/// Inside the framework's own crates the tokens come back untouched and a
/// `compile_error!` names the missing dependency instead. Call it once, on what
/// a decorator is about to return.
pub fn reroot(tokens: TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();

    let Some(u) = umbrella() else {
        // One traversal for both jobs: collecting the roots is the same walk as
        // rewriting them, and keeping it that way is why a root that only ever
        // appears inside a `crate = "…"` literal still reports.
        let mut found = Vec::new();
        walk(&trees, None, "", false, &mut found);
        let mut out = TokenStream::from_iter(trees);
        out.extend(missing_dependency_error(found));
        return out;
    };

    let prefix = root_prefix(u);
    let prefix: Vec<TokenTree> = prefix.into_iter().collect();
    // The text form the derive-relocation attributes parse — `crate = "…"`
    // takes the path as a string, so it cannot reuse the tokens.
    let prefix_text: String = prefix.iter().map(ToString::to_string).collect();

    let mut sink = Vec::new();
    match walk(&trees, Some(&prefix), &prefix_text, false, &mut sink) {
        Some(rewritten) => TokenStream::from_iter(rewritten),
        None => TokenStream::from_iter(trees),
    }
}

/// One walk, two jobs: re-root when `prefix` is `Some`, and record every root
/// seen into `found` either way.
///
/// Returns `None` when nothing under `trees` changed, so an untouched subtree
/// is reused rather than rebuilt — most of a decorator's output is the
/// developer's own item, which has no framework paths in it at all.
///
/// `in_crate_attr` marks the inside of a `#[serde(…)]` / `#[schemars(…)]`
/// group, the only place a path is carried as a string literal. Outside one,
/// literals are left alone without stringifying them.
fn walk(
    trees: &[TokenTree],
    prefix: Option<&[TokenTree]>,
    prefix_text: &str,
    in_crate_attr: bool,
    found: &mut Vec<String>,
) -> Option<Vec<TokenTree>> {
    let mut out: Option<Vec<TokenTree>> = None;
    let mut i = 0;

    while i < trees.len() {
        // A leading `::` is two puncts, the first joint. Only a *rooted* path
        // is rewritten: a bare `nest_rs_core` — a local binding, or an
        // unrooted `use nest_rs_core::…` — is not ours to touch.
        //
        // What precedes the `::` is deliberately **not** inspected. Three
        // rounds of trying said otherwise: `impl`/`dyn`/`as` are `Ident`s that
        // open a path, and `->`, `=>` and a plain comparison `>` all end in
        // `>` like the qualified path `<T as Trait>::assoc` does. The only
        // shape a prefix check would protect is an associated item literally
        // named `nest_rs_<x>`, which no emission has. Mid-path segments are
        // handled below instead, where the answer is certain.
        if let Some(ident) = segment_at(trees, i)
            && let Some(concern) = concern_of(&ident.to_string())
        {
            found.push(concern.clone());
            if let Some(prefix) = prefix {
                let buf = out.get_or_insert_with(|| trees[..i].to_vec());
                buf.extend(prefix.iter().cloned());
                buf.extend([
                    TokenTree::Punct(Punct::new(':', Spacing::Joint)),
                    TokenTree::Punct(Punct::new(':', Spacing::Alone)),
                    TokenTree::Ident(Ident::new(&concern, ident.span())),
                ]);
                i += 3;
                // The rest of *this* path is copied verbatim. A path has one
                // root, and `nest-rs-ws` re-exports `nest_rs_http`, so
                // `::nest_rs_ws::nest_rs_http::HttpEndpointMeta` must come out
                // as `::nest_rs::ws::nest_rs_http::…` — not with the umbrella
                // spliced into every segment.
                while segment_at(trees, i).is_some() {
                    buf.extend(trees[i..i + 3].iter().cloned());
                    i += 3;
                }
                continue;
            }
            i += 3;
            while segment_at(trees, i).is_some() {
                i += 3;
            }
            if let Some(buf) = out.as_mut() {
                buf.extend(trees[..i].iter().skip(buf.len()).cloned());
            }
            continue;
        }

        match &trees[i] {
            TokenTree::Group(g) => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                let nested = in_crate_attr || carries_a_crate_path(&inner);
                match walk(&inner, prefix, prefix_text, nested, found) {
                    Some(rewritten) => {
                        let mut rebuilt =
                            Group::new(g.delimiter(), TokenStream::from_iter(rewritten));
                        rebuilt.set_span(g.span());
                        out.get_or_insert_with(|| trees[..i].to_vec())
                            .push(TokenTree::Group(rebuilt));
                    }
                    None => {
                        if let Some(buf) = out.as_mut() {
                            buf.push(trees[i].clone());
                        }
                    }
                }
            }
            TokenTree::Literal(lit) if in_crate_attr => {
                match prefix.and_then(|_| rewrite_path_literal(lit, prefix_text, found)) {
                    Some(rewritten) => out
                        .get_or_insert_with(|| trees[..i].to_vec())
                        .push(TokenTree::Literal(rewritten)),
                    None => {
                        if prefix.is_none() {
                            record_literal_root(lit, found);
                        }
                        if let Some(buf) = out.as_mut() {
                            buf.push(trees[i].clone());
                        }
                    }
                }
            }
            other => {
                if let Some(buf) = out.as_mut() {
                    buf.push(other.clone());
                }
            }
        }
        i += 1;
    }

    out
}

/// Whether this group carries a `crate = ` override spelled as a string.
///
/// Matched by **shape, at any position**, rather than by a list of derive
/// names: `crate =` is what `#[serde(…)]`, `#[schemars(…)]` and async-graphql's
/// `#[Object(crate = "…")]` / `#[graphql(crate = "…")]` all spell, so a
/// newly-wrapped third-party macro is covered the day it lands instead of
/// silently falling back to the call site's prelude. The walk re-asks on every
/// group descent, so an override nested one group under its derive name arms
/// the literal branch when that group is reached. Validator's override is a
/// bare path and falls out of the ordinary segment branch either way.
fn carries_a_crate_path(inner: &[TokenTree]) -> bool {
    inner.windows(2).any(|pair| {
        matches!(
            (&pair[0], &pair[1]),
            (TokenTree::Ident(id), TokenTree::Punct(p)) if id == "crate" && p.as_char() == '='
        )
    })
}

/// The single `compile_error!` naming every concern the call site cannot reach.
///
/// Empty when nothing is missing, which is the framework's own crates: they
/// reach each concern by its sibling name already.
fn missing_dependency_error(mut concerns: Vec<String>) -> TokenStream {
    concerns.sort();
    concerns.dedup();
    concerns.retain(|c| sibling_missing(c));
    if concerns.is_empty() {
        return TokenStream::new();
    }

    // `core` is not a feature — it always ships with the umbrella — so it
    // counts as missing without naming one.
    let features: Vec<String> = concerns
        .iter()
        .filter(|c| *c != "core")
        .map(|c| format!("\"{}\"", c.replace('_', "-")))
        .collect();
    // The framework's own version, so the line is copy-pasteable rather than a
    // placeholder the developer has to look up.
    let version = env!("CARGO_PKG_VERSION");
    let req = version
        .rsplit_once('.')
        .map_or(version, |(major_minor, _)| major_minor);
    let line = if features.is_empty() {
        format!("nest-rs = \"{req}\"")
    } else {
        format!(
            "nest-rs = {{ version = \"{req}\", features = [{}] }}",
            features.join(", ")
        )
    };
    let msg = format!(
        "this nestrs decorator expands into the framework, which this crate cannot reach. \
         Add to Cargo.toml:\n\n    {line}"
    );
    quote! { ::std::compile_error!(#msg); }
}

/// A string literal holding a `::nest_rs_<concern>::…` path, re-rooted.
///
/// `None` for every other literal — a message that merely mentions a crate name
/// is not a path, so the match is anchored at the start and requires the
/// leading `::` an emitted path always carries.
fn rewrite_path_literal(
    lit: &Literal,
    prefix_text: &str,
    found: &mut Vec<String>,
) -> Option<Literal> {
    let (concern, tail) = split_path_literal(lit)?;
    found.push(concern.clone());
    let mut out = Literal::string(&format!("{prefix_text}::{concern}{tail}"));
    out.set_span(lit.span());
    Some(out)
}

/// The diagnostic path's half of [`rewrite_path_literal`] — record the root
/// without building a replacement.
fn record_literal_root(lit: &Literal, found: &mut Vec<String>) {
    if let Some((concern, _)) = split_path_literal(lit) {
        found.push(concern);
    }
}

fn split_path_literal(lit: &Literal) -> Option<(String, String)> {
    let raw = lit.to_string();
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let rest = inner.strip_prefix("::")?;
    let (head, tail) = rest.split_at(rest.find("::").unwrap_or(rest.len()));
    Some((concern_of(head)?, tail.to_owned()))
}

/// `:: <ident>` at `i`, the shape every path segment takes.
fn segment_at(trees: &[TokenTree], i: usize) -> Option<&Ident> {
    let (TokenTree::Punct(first), TokenTree::Punct(second), TokenTree::Ident(ident)) =
        (trees.get(i)?, trees.get(i + 1)?, trees.get(i + 2)?)
    else {
        return None;
    };
    (first.as_char() == ':' && first.spacing() == Spacing::Joint && second.as_char() == ':')
        .then_some(ident)
}

/// The concern a sibling crate name denotes, or `None` if it names none.
///
/// The name after the prefix is exactly what `nest-rs` re-exports the crate as
/// (`nest_rs_exception_filters` ⇒ `nest_rs::exception_filters`), so no mapping
/// table can drift out of sync with the facade. `::nest_rs_` alone is not a
/// concern, and the umbrella has no module for the macro crates — neither ever
/// appears in an expansion, but a typo here would silently produce an
/// unresolvable path rather than a loud one.
fn concern_of(name: &str) -> Option<String> {
    let concern = name.strip_prefix("nest_rs_")?;
    (!concern.is_empty() && !concern.ends_with("_macros")).then(|| concern.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn rewritten(input: TokenStream) -> String {
        let prefix: Vec<TokenTree> = quote!(::nest_rs).into_iter().collect();
        let trees: Vec<TokenTree> = input.into_iter().collect();
        let mut found = Vec::new();
        match walk(&trees, Some(&prefix), "::nest_rs", false, &mut found) {
            Some(out) => TokenStream::from_iter(out).to_string(),
            None => TokenStream::from_iter(trees).to_string(),
        }
    }

    fn roots_seen(input: TokenStream) -> Vec<String> {
        let trees: Vec<TokenTree> = input.into_iter().collect();
        let mut found = Vec::new();
        walk(&trees, None, "", false, &mut found);
        found
    }

    #[test]
    fn roots_a_path_held_in_a_string_literal() {
        // serde and schemars take their `crate = ` override as a string.
        assert_eq!(
            rewritten(quote!(#[serde(crate = "::nest_rs_http::serde")])),
            quote!(#[serde(crate = "::nest_rs::http::serde")]).to_string()
        );
    }

    #[test]
    fn a_literal_only_root_still_reports_as_missing() {
        // The diagnostic walk and the rewrite walk are the same walk — an
        // expansion whose only framework root lives in a `crate = "…"` string
        // used to report nothing at all.
        assert_eq!(
            roots_seen(quote!(#[serde(crate = "::nest_rs_http::serde")])),
            vec!["http".to_owned()]
        );
    }

    #[test]
    fn leaves_an_ordinary_string_alone() {
        let untouched = quote!(compile_error!("nest_rs_core is missing"));
        assert_eq!(rewritten(untouched.clone()), untouched.to_string());
    }

    #[test]
    fn leaves_a_doc_comment_alone() {
        // Doc comments are `#[doc = "…"]`, not a `crate = ` attribute, so the
        // literal branch never even stringifies them.
        let untouched = quote!(#[doc = "see ::nest_rs_core::Container"]);
        assert_eq!(rewritten(untouched.clone()), untouched.to_string());
    }

    #[test]
    fn roots_a_sibling_path_at_the_umbrella() {
        assert_eq!(
            rewritten(quote!(::nest_rs_core::Container)),
            quote!(::nest_rs::core::Container).to_string()
        );
    }

    #[test]
    fn roots_a_path_that_follows_a_keyword() {
        // `impl`, `dyn` and `as` are `Ident` tokens, so "the previous token is
        // an ident" cannot mean "mid-path" — this is the shape every
        // `Discoverable` expansion opens with.
        assert_eq!(
            rewritten(quote!(impl ::nest_rs_core::Discoverable for T {})),
            quote!(impl ::nest_rs::core::Discoverable for T {}).to_string()
        );
    }

    #[test]
    fn rewrites_only_the_root_of_a_re_exported_path() {
        // `nest-rs-ws` re-exports `nest_rs_http`, and `#[messages]` reaches
        // `HttpEndpointMeta` through it. Splicing the umbrella into the second
        // segment produced `::nest_rs::ws::nest_rs::http::…` — caught by
        // `nest-rs-macro-hygiene` rather than by a user.
        assert_eq!(
            rewritten(quote!(::nest_rs_ws::nest_rs_http::HttpEndpointMeta)),
            quote!(::nest_rs::ws::nest_rs_http::HttpEndpointMeta).to_string()
        );
    }

    #[test]
    fn roots_a_return_type() {
        // `->` ends in `>`, which also closes a qualified path. Treating the
        // two alike left every `Discoverable::register` return type pointing
        // at a crate the app never declared.
        assert_eq!(
            rewritten(quote! {
                fn register(b: ::nest_rs_core::ContainerBuilder) -> ::nest_rs_core::ContainerBuilder
            }),
            quote! {
                fn register(b: ::nest_rs::core::ContainerBuilder) -> ::nest_rs::core::ContainerBuilder
            }
            .to_string()
        );
    }

    #[test]
    fn roots_a_match_arm_body() {
        // `=>` ends in `>` too — the fallback arm of the `#[messages]` dispatch
        // table is the one path this missed after the `->` fix.
        assert_eq!(
            rewritten(quote!(match m {
                __other => ::nest_rs_ws::WsReply::unknown(__other),
            })),
            quote!(match m {
                __other => ::nest_rs::ws::WsReply::unknown(__other),
            })
            .to_string()
        );
    }

    #[test]
    fn roots_a_path_after_any_operator() {
        // Everything that ends in `>` was tried as "this continues a path" and
        // each attempt broke a real emission: `->` a return type, `=>` a match
        // arm, and a bare `>` the `if __v > ::nest_rs_queue::MAX` bound in
        // `#[processor]`. What precedes the `::` is no longer inspected.
        assert_eq!(
            rewritten(quote!(if __v > ::nest_rs_queue::MAX {})),
            quote!(if __v > ::nest_rs::queue::MAX {}).to_string()
        );
    }

    #[test]
    fn rewrites_inside_groups_and_generics() {
        assert_eq!(
            rewritten(quote! {
                fn f(c: &::nest_rs_core::Container) -> ::std::vec::Vec<::nest_rs_guards::Guard> {}
            }),
            quote! {
                fn f(c: &::nest_rs::core::Container) -> ::std::vec::Vec<::nest_rs::guards::Guard> {}
            }
            .to_string()
        );
    }

    #[test]
    fn keeps_multi_word_concerns_aligned_with_the_facade() {
        assert_eq!(
            rewritten(quote!(::nest_rs_exception_filters::ExceptionFilter)),
            quote!(::nest_rs::exception_filters::ExceptionFilter).to_string()
        );
    }

    #[test]
    fn leaves_std_and_third_party_roots_alone() {
        let untouched = quote!(::std::sync::Arc<::serde::Serialize>);
        assert_eq!(rewritten(untouched.clone()), untouched.to_string());
    }

    #[test]
    fn ignores_an_unrooted_ident_that_merely_looks_like_a_crate() {
        // A local binding or a `use` alias — rewriting it would capture the
        // developer's own name.
        let untouched = quote!(let nest_rs_core = 1;);
        assert_eq!(rewritten(untouched.clone()), untouched.to_string());
    }

    #[test]
    fn an_expansion_with_no_framework_path_is_returned_unchanged() {
        // `None` from the walk means "reuse the input" — most of a decorator's
        // output is the developer's own item, which has no roots in it.
        let trees: Vec<TokenTree> = quote!(
            pub struct Bare {
                pub name: String,
            }
        )
        .into_iter()
        .collect();
        let prefix: Vec<TokenTree> = quote!(::nest_rs).into_iter().collect();
        let mut found = Vec::new();
        assert!(walk(&trees, Some(&prefix), "::nest_rs", false, &mut found).is_none());
        assert!(found.is_empty());
    }
}
