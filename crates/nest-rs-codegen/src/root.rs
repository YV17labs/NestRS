//! Where an expansion's framework paths are rooted.
//!
//! An expansion resolves against the *developer's* extern prelude: an app
//! declares only the umbrella (`::nest_rs::<concern>`), while the framework's
//! own crates cannot (`nest-rs` depends on them) and need `::nest_rs_<concern>`.
//! Macros emit the sibling form; [`reroot`] rewrites the finished stream.
//!
//! The walk sees the developer's own item too, so a hand-written
//! `::nest_rs_http::X` there is re-rooted as well; it resolves either way.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use proc_macro_crate::{Error, FoundCrate, crate_name};
use proc_macro2::{Group, Ident, Literal, Spacing, Span, TokenStream, TokenTree};
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

/// Whether each `nest-rs-<concern>` sibling is declared, memoized: `crate_name`
/// stats the filesystem on every call.
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

/// `true` when the manifest was readable and holds no such sibling; a build
/// system `proc-macro-crate` cannot introspect answers `false`.
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
        // One walk for both jobs, so a root only inside a `crate = "…"` reports.
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
/// is reused rather than rebuilt.
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
        // Only a *rooted* path is rewritten: a bare `nest_rs_core` may be a
        // local binding. What precedes the `::` is deliberately not inspected:
        // `impl`/`dyn`/`as` open a path, and `->`, `=>` and `>` all end in `>`
        // like `<T as Trait>::assoc` does.
        if let Some(ident) = segment_at(trees, i)
            && let Some(concern) = concern_of(&ident.to_string())
        {
            found.push(concern.clone());
            if let Some(prefix) = prefix {
                // Keeps the replaced root's span, or errors move to the attribute.
                let span = trees[i].span();
                let buf = out.get_or_insert_with(|| trees[..i].to_vec());
                buf.extend(prefix.iter().cloned().map(|mut tree| {
                    tree.set_span(span);
                    tree
                }));
                buf.extend(trees[i..i + 2].iter().cloned());
                buf.push(TokenTree::Ident(Ident::new(&concern, ident.span())));
                i += 3;
                // One root per path: `::nest_rs_ws::nest_rs_http::X` becomes
                // `::nest_rs::ws::nest_rs_http::X`.
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
/// Matched by shape at any position, not by derive name, so a newly-wrapped
/// third-party macro spelling `crate = "…"` is covered too.
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
/// Empty when nothing is missing.
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
/// `None` for every other literal: the match is anchored at a leading `::`.
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
/// The name after the prefix is what `nest-rs` re-exports the crate as
/// (`nest_rs_exception_filters` ⇒ `nest_rs::exception_filters`).
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
        assert_eq!(
            rewritten(quote!(#[serde(crate = "::nest_rs_http::serde")])),
            quote!(#[serde(crate = "::nest_rs::http::serde")]).to_string()
        );
    }

    #[test]
    fn a_literal_only_root_still_reports_as_missing() {
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
        assert_eq!(
            rewritten(quote!(impl ::nest_rs_core::Discoverable for T {})),
            quote!(impl ::nest_rs::core::Discoverable for T {}).to_string()
        );
    }

    #[test]
    fn rewrites_only_the_root_of_a_re_exported_path() {
        assert_eq!(
            rewritten(quote!(::nest_rs_ws::nest_rs_http::HttpEndpointMeta)),
            quote!(::nest_rs::ws::nest_rs_http::HttpEndpointMeta).to_string()
        );
    }

    #[test]
    fn roots_a_return_type() {
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
        let untouched = quote!(let nest_rs_core = 1;);
        assert_eq!(rewritten(untouched.clone()), untouched.to_string());
    }

    #[test]
    fn an_expansion_with_no_framework_path_is_returned_unchanged() {
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
