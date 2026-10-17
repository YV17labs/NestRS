//! Where an expansion's framework paths are rooted.
//!
//! An expansion resolves against the *developer's* extern prelude: an app
//! declares only the umbrella (`::nest_rs::<concern>`), while the framework's
//! own crates cannot (`nest-rs` depends on them) and need `::nest_rs_<concern>`.
//! Macros emit the sibling form; [`reroot`] rewrites the finished stream.
//!
//! A framework crate is recognised by one closed table, [`FRAMEWORK`], and by
//! nothing else: a community crate's root (`::nestrs_kafka`, or one that took
//! the framework's prefix) is never rewritten.
//!
//! The walk sees the developer's own item too, so a hand-written
//! `::nest_rs_http::X` there is re-rooted as well; it resolves either way.
//!
//! A crate an expansion reaches only because the developer wrote an attribute
//! (`#[authorize]`, `#[crud]`) ships behind a feature the umbrella may lack, so
//! [`reroot`] follows the expansion with the umbrella's `__requires_<feature>!`
//! for it: nothing when the feature is on, the sentence naming it when it is off.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use proc_macro_crate::{Error, FoundCrate, crate_name};
use proc_macro2::{Delimiter, Group, Ident, Literal, Spacing, Span, TokenStream, TokenTree};
use quote::quote;

use crate::attrs::cfg_attr_conditions;

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

/// Whether each framework crate is declared, memoized by its library name:
/// `crate_name` stats the filesystem on every call.
static SIBLINGS: OnceLock<Mutex<HashMap<&'static str, bool>>> = OnceLock::new();

/// One first-party crate the umbrella re-exports.
#[derive(Debug, PartialEq, Eq)]
struct FrameworkCrate {
    /// Its library name, the root an expansion emits (`nest_rs_oauth_client`).
    lib: &'static str,
    /// Where the umbrella re-exports it (`oauth::client`).
    path: &'static str,
    /// The umbrella feature that ships it; `None` for the kernel, always there.
    feature: Option<&'static str>,
    /// The attribute whose expansion reaches this crate when no decorator's
    /// own does, so its feature is the developer's to enable; `None` for a
    /// crate every decorator emitting it ships with.
    gated_by: Option<&'static str>,
}

const fn row(lib: &'static str, path: &'static str, feature: &'static str) -> FrameworkCrate {
    FrameworkCrate {
        lib,
        path,
        feature: Some(feature),
        gated_by: None,
    }
}

/// A crate an expansion reaches only through `attribute`, written by the
/// developer on a decorator that otherwise ships without it.
const fn gated(
    lib: &'static str,
    path: &'static str,
    feature: &'static str,
    attribute: &'static str,
) -> FrameworkCrate {
    FrameworkCrate {
        lib,
        path,
        feature: Some(feature),
        gated_by: Some(attribute),
    }
}

/// Every first-party crate, one row per crate `nest-rs`'s `lib.rs` re-exports:
/// the only place a framework crate is recognised.
///
/// A row an expansion emits is held by `nest-rs-macro-hygiene`, which builds
/// every decorator through the umbrella alone; the rows no macro emits yet are
/// held by this file's tests.
const FRAMEWORK: &[FrameworkCrate] = &[
    FrameworkCrate {
        lib: "nest_rs_core",
        path: "core",
        feature: None,
        gated_by: None,
    },
    row("nest_rs_http", "http", "http"),
    row("nest_rs_config", "config", "config"),
    row("nest_rs_database", "database", "database"),
    gated(
        "nest_rs_seaorm",
        "seaorm",
        "seaorm",
        "`#[crud]` or `#[authorize(…, bind = …)]`",
    ),
    row("nest_rs_graphql", "graphql", "graphql"),
    row("nest_rs_ws", "ws", "ws"),
    row("nest_rs_mcp", "mcp", "mcp"),
    row("nest_rs_queue", "queue", "queue"),
    row("nest_rs_redis", "redis", "redis"),
    row("nest_rs_schedule", "schedule", "schedule"),
    row("nest_rs_events", "events", "events"),
    row("nest_rs_authn", "authn", "authn"),
    gated("nest_rs_authz", "authz", "authz", "`#[authorize]`"),
    row("nest_rs_oauth_client", "oauth::client", "oauth-client"),
    row(
        "nest_rs_oauth_resource",
        "oauth::resource",
        "oauth-resource",
    ),
    row("nest_rs_oauth_server", "oauth::server", "oauth-server"),
    row("nest_rs_social", "social", "social"),
    row("nest_rs_opentelemetry", "opentelemetry", "opentelemetry"),
    row("nest_rs_openapi", "openapi", "openapi"),
    row("nest_rs_health", "health", "health"),
    row("nest_rs_throttler", "throttler", "throttler"),
    row("nest_rs_server_timing", "server_timing", "server-timing"),
    row("nest_rs_static_files", "static_files", "static-files"),
    row("nest_rs_testing", "testing", "testing"),
    row("nest_rs_guards", "guards", "guards"),
    row("nest_rs_pipes", "pipes", "pipes"),
    row("nest_rs_interceptors", "interceptors", "interceptors"),
    row("nest_rs_filters", "filters", "filters"),
    row(
        "nest_rs_exception_filters",
        "exception_filters",
        "exception-filters",
    ),
    row("nest_rs_storage", "storage", "storage"),
];

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

/// `true` when the manifest was readable and does not declare `krate`; a build
/// system `proc-macro-crate` cannot introspect answers `false`.
fn sibling_missing(krate: &FrameworkCrate) -> bool {
    let cache = SIBLINGS.get_or_init(Default::default);
    if let Ok(map) = cache.lock()
        && let Some(known) = map.get(krate.lib)
    {
        return *known;
    }
    let name = krate.lib.replace('_', "-");
    let missing = matches!(crate_name(&name), Err(Error::CrateNotFound { .. }));
    if let Ok(mut map) = cache.lock() {
        map.insert(krate.lib, missing);
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

/// Rewrite every framework root (`::nest_rs_<concern>`) in a finished expansion
/// to the path the call site can actually resolve.
///
/// Inside the framework's own crates the tokens come back untouched and a
/// `compile_error!` names the missing dependency instead. Behind the umbrella,
/// a crate only an attribute pulls in (`#[authorize]`, `#[crud]`) is followed
/// by the umbrella's check for its feature, an item: the output belongs in
/// item or associated-item position. Call it once, on what a decorator is
/// about to return.
pub fn reroot(tokens: TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();

    let Some(u) = umbrella() else {
        // One walk for both jobs, so a root only inside a `crate = "…"` reports.
        let mut found = Vec::new();
        walk(&trees, None, "", false, &[], &mut found);
        let mut out = TokenStream::from_iter(trees);
        out.extend(missing_dependency_error(
            found.into_iter().map(|root| root.krate).collect(),
        ));
        return out;
    };

    let prefix: Vec<TokenTree> = root_prefix(u).into_iter().collect();
    rerooted(trees, &prefix)
}

/// `trees` with every framework root under `prefix`, followed by the
/// umbrella's `__requires_<feature>!` for each attribute-gated crate reached.
fn rerooted(trees: Vec<TokenTree>, prefix: &[TokenTree]) -> TokenStream {
    // The text form the derive-relocation attributes parse — `crate = "…"`
    // takes the path as a string, so it cannot reuse the tokens.
    let prefix_text: String = prefix.iter().map(ToString::to_string).collect();

    let mut found = Vec::new();
    let mut out = match walk(&trees, Some(prefix), &prefix_text, false, &[], &mut found) {
        Some(rewritten) => TokenStream::from_iter(rewritten),
        None => TokenStream::from_iter(trees),
    };
    out.extend(feature_checks(found, prefix));
    out
}

/// One `<umbrella>::__requires_<feature>! { "…" }` per attribute-gated crate in
/// `found`, under the `#[cfg]`s enclosing the paths that reach it: the umbrella
/// expands it to nothing when it ships the feature, and to the sentence as a
/// compile error when it does not. A method compiled out takes its check with
/// it, as it takes its wrapper.
///
/// Every caller of [`reroot`] is an attribute or a derive, whose output sits in
/// item position, where an invocation expanding to nothing is legal.
fn feature_checks(found: Vec<Root>, prefix: &[TokenTree]) -> TokenStream {
    let mut checks: Vec<(&'static FrameworkCrate, String, Vec<TokenStream>)> = found
        .into_iter()
        .filter(|root| root.krate.gated_by.is_some())
        .map(|root| {
            let key = root.cfgs.iter().map(ToString::to_string).collect();
            (root.krate, key, root.cfgs)
        })
        .collect();
    // Sorted, a crate's unconditional check (the empty key) comes first: a crate
    // one path reaches unconditionally is checked once, unconditionally.
    checks.sort_by(|a, b| (a.0.lib, &a.1).cmp(&(b.0.lib, &b.1)));
    checks.dedup_by(|later, kept| {
        later.0.lib == kept.0.lib && (kept.1.is_empty() || later.1 == kept.1)
    });
    checks
        .into_iter()
        .filter_map(|(krate, _, cfgs)| {
            let (feature, attribute) = (krate.feature?, krate.gated_by?);
            let check = Ident::new(
                &format!("__requires_{}", feature.replace('-', "_")),
                Span::call_site(),
            );
            let sentence = missing_feature_message(krate.path, feature, attribute);
            let prefix = prefix.iter();
            Some(quote! { #(#[#cfgs])* #(#prefix)* :: #check ! { #sentence } })
        })
        .collect()
}

/// A framework crate a walk met, with the `cfg`s enclosing the path that
/// reaches it, outermost first.
struct Root {
    krate: &'static FrameworkCrate,
    cfgs: Vec<TokenStream>,
}

impl Root {
    /// `krate` met under `scope`'s `cfg`s and those `pending` at its own level,
    /// kept only for an attribute-gated crate: no check reads another's.
    fn new(krate: &'static FrameworkCrate, scope: &[TokenStream], pending: &[TokenStream]) -> Self {
        let cfgs = if krate.gated_by.is_some() {
            enclosing(scope, pending)
        } else {
            Vec::new()
        };
        Self { krate, cfgs }
    }
}

/// What of an outer attribute's content decides whether its item is compiled:
/// `cfg(…)` whole, `cfg_attr(P, …)` kept to its `cfg(…)`s, nothing else.
fn cfg_part(attribute: &Group) -> Option<TokenStream> {
    let mut trees = attribute.stream().into_iter();
    let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(args)), None) =
        (trees.next(), trees.next(), trees.next())
    else {
        return None;
    };
    if args.delimiter() != Delimiter::Parenthesis {
        return None;
    }
    if name == "cfg" {
        Some(attribute.stream())
    } else if name == "cfg_attr" {
        cfg_attr_conditions(args.stream())
    } else {
        None
    }
}

/// What a build lacking `feature` says of an expansion `attribute` pulled into
/// `nest_rs::<path>`, ending on the umbrella line that enables it.
fn missing_feature_message(path: &str, feature: &str, attribute: &str) -> String {
    format!(
        "{attribute} expands into `nest_rs::{path}`, which this build does not enable. \
         Add the feature to Cargo.toml:\n\n    {}",
        install_line(&[feature])
    )
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
///
/// `scope` holds the `cfg`s enclosing `trees`. A `#[cfg]` met here covers what
/// follows it up to the end of its item or statement: a `;`, or a `{ … }` at
/// this level that [`closes_run`].
fn walk(
    trees: &[TokenTree],
    prefix: Option<&[TokenTree]>,
    prefix_text: &str,
    in_crate_attr: bool,
    scope: &[TokenStream],
    found: &mut Vec<Root>,
) -> Option<Vec<TokenTree>> {
    let mut out: Option<Vec<TokenTree>> = None;
    let mut pending: Vec<TokenStream> = Vec::new();
    // Where the item or statement `pending` covers opens: its first `#[cfg]`.
    let mut run_start = 0;
    let mut i = 0;

    while i < trees.len() {
        if let (TokenTree::Punct(hash), Some(TokenTree::Group(attribute))) =
            (&trees[i], trees.get(i + 1))
            && hash.as_char() == '#'
            && attribute.delimiter() == Delimiter::Bracket
            && let Some(cfg) = cfg_part(attribute)
        {
            if pending.is_empty() {
                run_start = i;
            }
            pending.push(cfg);
        }

        // Only a *rooted* path is rewritten: a bare `nest_rs_core` may be a
        // local binding. What precedes the `::` is deliberately not inspected:
        // `impl`/`dyn`/`as` open a path, and `->`, `=>` and `>` all end in `>`
        // like `<T as Trait>::assoc` does.
        if let Some(ident) = segment_at(trees, i)
            && let Some(krate) = framework_crate(&ident.to_string())
        {
            found.push(Root::new(krate, scope, &pending));
            if let Some(prefix) = prefix {
                // Keeps the replaced root's span, or errors move to the attribute.
                let span = trees[i].span();
                let buf = out.get_or_insert_with(|| trees[..i].to_vec());
                buf.extend(prefix.iter().cloned().map(|mut tree| {
                    tree.set_span(span);
                    tree
                }));
                for segment in krate.path.split("::") {
                    buf.extend(trees[i..i + 2].iter().cloned());
                    buf.push(TokenTree::Ident(Ident::new(segment, ident.span())));
                }
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
                let cfgs = if pending.is_empty() {
                    Cow::Borrowed(scope)
                } else {
                    Cow::Owned(enclosing(scope, &pending))
                };
                match walk(&inner, prefix, prefix_text, nested, &cfgs, found) {
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
                if g.delimiter() == Delimiter::Brace
                    && !pending.is_empty()
                    && closes_run(trees, run_start, i)
                {
                    pending.clear();
                }
            }
            TokenTree::Literal(lit) if in_crate_attr => {
                let root = split_path_literal(lit);
                if let Some((krate, _)) = &root {
                    found.push(Root::new(krate, scope, &pending));
                }
                match root.filter(|_| prefix.is_some()) {
                    Some((krate, tail)) => {
                        out.get_or_insert_with(|| trees[..i].to_vec())
                            .push(TokenTree::Literal(rerooted_literal(
                                lit,
                                prefix_text,
                                krate,
                                &tail,
                            )))
                    }
                    None => {
                        if let Some(buf) = out.as_mut() {
                            buf.push(trees[i].clone());
                        }
                    }
                }
            }
            other => {
                if matches!(other, TokenTree::Punct(p) if p.as_char() == ';') {
                    pending.clear();
                }
                if let Some(buf) = out.as_mut() {
                    buf.push(other.clone());
                }
            }
        }
        i += 1;
    }

    out
}

/// Whether the `{ … }` at `brace` ends the item or statement whose attributes
/// open at `start`. A `let` runs on to its `;`; anything else ends at a brace
/// that nothing after it continues, so `else`, `as`, an operator or another
/// group keeps it open (`if match v { … } { … } else { … }`). Reading too far
/// only drops a check from a build; stopping short would refuse one.
fn closes_run(trees: &[TokenTree], start: usize, brace: usize) -> bool {
    let mut first = start;
    while let (Some(TokenTree::Punct(hash)), Some(TokenTree::Group(attribute))) =
        (trees.get(first), trees.get(first + 1))
        && hash.as_char() == '#'
        && attribute.delimiter() == Delimiter::Bracket
    {
        first += 2;
    }
    if matches!(trees.get(first), Some(TokenTree::Ident(word)) if word == "let") {
        return false;
    }
    match trees.get(brace + 1) {
        None | Some(TokenTree::Literal(_)) => true,
        Some(TokenTree::Ident(next)) => next != "else" && next != "as",
        Some(TokenTree::Punct(next)) => matches!(next.as_char(), '#' | ',' | ';' | ':'),
        Some(TokenTree::Group(_)) => false,
    }
}

/// The `cfg`s enclosing what a walk meets: its scope's, then those met at its
/// own level.
fn enclosing(scope: &[TokenStream], pending: &[TokenStream]) -> Vec<TokenStream> {
    scope.iter().chain(pending).cloned().collect()
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

/// The single `compile_error!` naming every framework crate the call site
/// cannot reach.
///
/// Empty when nothing is missing.
fn missing_dependency_error(found: Vec<&'static FrameworkCrate>) -> TokenStream {
    match missing_dependency_message(found) {
        Some(msg) => quote! { ::std::compile_error!(#msg); },
        None => TokenStream::new(),
    }
}

/// The sentence [`missing_dependency_error`] says, naming the umbrella line
/// that ships every crate in `found` the manifest does not declare.
fn missing_dependency_message(mut found: Vec<&'static FrameworkCrate>) -> Option<String> {
    found.sort_by_key(|krate| krate.lib);
    found.dedup();
    found.retain(|krate| sibling_missing(krate));
    if found.is_empty() {
        return None;
    }

    // The kernel names no feature — it always ships with the umbrella — so it
    // counts as missing without one.
    let features: Vec<&str> = found.iter().filter_map(|krate| krate.feature).collect();
    Some(format!(
        "this nestrs decorator expands into the framework, which this crate cannot reach. \
         Add to Cargo.toml:\n\n    {}",
        install_line(&features)
    ))
}

/// The `nest-rs` line enabling `features`, at the `major.minor` this crate
/// ships in lockstep with the umbrella.
fn install_line(features: &[&str]) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let req = version
        .rsplit_once('.')
        .map_or(version, |(major_minor, _)| major_minor);
    if features.is_empty() {
        return format!("nest-rs = \"{req}\"");
    }
    let features: Vec<String> = features.iter().map(|f| format!("\"{f}\"")).collect();
    format!(
        "nest-rs = {{ version = \"{req}\", features = [{}] }}",
        features.join(", ")
    )
}

/// `lit`, a string holding `krate`'s `::nest_rs_<concern>…` path followed by
/// `tail`, re-rooted under `prefix_text`.
fn rerooted_literal(
    lit: &Literal,
    prefix_text: &str,
    krate: &FrameworkCrate,
    tail: &str,
) -> Literal {
    let mut out = Literal::string(&format!("{prefix_text}::{}{tail}", krate.path));
    out.set_span(lit.span());
    out
}

fn split_path_literal(lit: &Literal) -> Option<(&'static FrameworkCrate, String)> {
    let raw = lit.to_string();
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    let rest = inner.strip_prefix("::")?;
    let (head, tail) = rest.split_at(rest.find("::").unwrap_or(rest.len()));
    Some((framework_crate(head)?, tail.to_owned()))
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

/// The [`FRAMEWORK`] row a root names, or `None` for any other crate.
fn framework_crate(name: &str) -> Option<&'static FrameworkCrate> {
    if !name.starts_with("nest_rs_") {
        return None;
    }
    FRAMEWORK.iter().find(|krate| krate.lib == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn rewritten(input: TokenStream) -> String {
        let prefix: Vec<TokenTree> = quote!(::nest_rs).into_iter().collect();
        let trees: Vec<TokenTree> = input.into_iter().collect();
        let mut found = Vec::new();
        match walk(&trees, Some(&prefix), "::nest_rs", false, &[], &mut found) {
            Some(out) => TokenStream::from_iter(out).to_string(),
            None => TokenStream::from_iter(trees).to_string(),
        }
    }

    fn roots_seen(input: TokenStream) -> Vec<&'static FrameworkCrate> {
        let trees: Vec<TokenTree> = input.into_iter().collect();
        let mut found = Vec::new();
        walk(&trees, None, "", false, &[], &mut found);
        found.into_iter().map(|root| root.krate).collect()
    }

    /// `input` rerooted at `::nest_rs`, its feature checks included.
    fn checked(input: TokenStream) -> String {
        let prefix: Vec<TokenTree> = quote!(::nest_rs).into_iter().collect();
        rerooted(input.into_iter().collect(), &prefix).to_string()
    }

    /// The umbrella's check for `feature`, under `cfgs`.
    fn check(feature: &str, cfgs: TokenStream) -> String {
        let krate = FRAMEWORK
            .iter()
            .find(|krate| krate.feature == Some(feature))
            .expect("a framework feature");
        let sentence = missing_feature_message(
            krate.path,
            feature,
            krate.gated_by.expect("an attribute-gated crate"),
        );
        let macro_name = Ident::new(&format!("__requires_{feature}"), Span::call_site());
        quote!(#cfgs ::nest_rs::#macro_name! { #sentence }).to_string()
    }

    #[test]
    fn an_attribute_gated_crate_is_checked_once_after_the_expansion() {
        let out = checked(quote! {
            fn read(ctx: &Ctx) -> Result<(), Error> {
                ::nest_rs_authz::class_gate::<::nest_rs_authz::Read, Note>(ctx)?;
                ::nest_rs_authz::graphql::masked_value_for::<::nest_rs_authz::Read, Note, _>(ctx, 0)
            }
        });
        assert!(out.ends_with(&check("authz", quote!())), "{out}");
        assert_eq!(out.matches("__requires_").count(), 1, "{out}");
    }

    #[test]
    fn a_crate_every_decorator_ships_with_is_not_checked() {
        let out = checked(quote!(
            fn f() -> ::nest_rs_http::Response {}
        ));
        assert!(!out.contains("__requires_"), "{out}");
    }

    #[test]
    fn a_crate_named_in_a_crate_attribute_is_checked_too() {
        let out = checked(quote!(
            #[serde(crate = "::nest_rs_seaorm::__private::serde")]
            struct Note;
        ));
        assert!(out.ends_with(&check("seaorm", quote!())), "{out}");
    }

    #[test]
    fn a_check_travels_with_the_cfg_around_its_path() {
        let out = checked(quote! {
            impl NotesResolver {
                #[cfg(feature = "admin")]
                fn purge(&self) {
                    ::nest_rs_authz::class_gate::<Delete, Note>(ctx)?;
                }
            }
        });
        assert!(
            out.ends_with(&check("authz", quote!(#[cfg(feature = "admin")]))),
            "{out}"
        );
    }

    #[test]
    fn a_cfg_attr_travels_as_its_cfg_alone() {
        let out = checked(quote! {
            #[cfg_attr(all(), cfg(any()), allow(dead_code))]
            fn hidden() { ::nest_rs_authz::X }
        });
        assert!(
            out.ends_with(&check("authz", quote!(#[cfg_attr(all(), cfg(any()))]))),
            "{out}"
        );

        let out = checked(quote! {
            #[cfg_attr(test, derive(Debug))]
            struct Shown(::nest_rs_authz::X);
        });
        assert!(out.ends_with(&check("authz", quote!())), "{out}");
    }

    #[test]
    fn a_nested_cfg_attr_travels_as_its_cfgs() {
        let out = checked(quote! {
            #[cfg_attr(all(), cfg_attr(any(), cfg(test)), inline)]
            fn hidden() { ::nest_rs_authz::X }
        });
        assert!(
            out.ends_with(&check(
                "authz",
                quote!(#[cfg_attr(all(), cfg_attr(any(), cfg(test)))])
            )),
            "{out}"
        );
    }

    #[test]
    fn a_cfg_ends_with_its_item() {
        let out = checked(quote! {
            #[cfg(feature = "admin")]
            use std::fmt::Debug;
            fn read() { ::nest_rs_authz::X }
        });
        assert!(out.ends_with(&check("authz", quote!())), "{out}");
    }

    #[test]
    fn a_path_outside_every_cfg_supersedes_the_checks_under_one() {
        let out = checked(quote! {
            #[cfg(feature = "admin")]
            fn purge() { ::nest_rs_authz::X }
            #[cfg(feature = "audit")]
            fn audit() { ::nest_rs_authz::X }
            fn read() { ::nest_rs_authz::X }
        });
        assert!(out.ends_with(&check("authz", quote!())), "{out}");
        assert_eq!(out.matches("__requires_").count(), 1, "{out}");
    }

    /// `#[routes]`' mount arm for a `#[version]`-ed route.
    #[test]
    fn a_cfg_covers_its_let_statement_past_the_braces_inside_it() {
        let out = checked(quote! {
            #[cfg(feature = "admin")]
            let () = if match __version {
                ::core::option::Option::Some(__v) => { ["2"].contains(&__v) }
                ::core::option::Option::None => true,
            } {
                __routes.route(Method::GET, &__path, ::nest_rs_authz::http::Authorize::new(ep));
            };
        });
        assert!(
            out.ends_with(&check("authz", quote!(#[cfg(feature = "admin")]))),
            "{out}"
        );
    }

    #[test]
    fn a_cfg_covers_an_if_statement_through_its_else_and_no_further() {
        let out = checked(quote! {
            #[cfg(feature = "admin")]
            if ready { serve() } else { ::nest_rs_authz::X }
        });
        assert!(
            out.ends_with(&check("authz", quote!(#[cfg(feature = "admin")]))),
            "{out}"
        );

        let out = checked(quote! {
            #[cfg(feature = "admin")]
            if ready { serve() } else { serve() }
            let _ = ::nest_rs_authz::X;
        });
        assert!(out.ends_with(&check("authz", quote!())), "{out}");
    }

    #[test]
    fn the_feature_sentence_names_the_attribute_the_concern_and_the_line() {
        let message = missing_feature_message("authz", "authz", "`#[authorize]`");
        let version = env!("CARGO_PKG_VERSION");
        let (major_minor, _) = version.rsplit_once('.').expect("a semver version");
        for part in [
            "`#[authorize]`",
            "`nest_rs::authz`",
            &format!(r#"nest-rs = {{ version = "{major_minor}", features = ["authz"] }}"#),
        ] {
            assert!(message.contains(part), "{message}");
        }
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
        let seen = roots_seen(quote!(#[serde(crate = "::nest_rs_http::serde")]));
        let libs: Vec<&str> = seen.iter().map(|row| row.lib).collect();
        assert_eq!(libs, ["nest_rs_http"]);
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
    fn leaves_a_community_crate_alone_whatever_its_prefix() {
        for untouched in [
            quote!(::nest_rs_kafka::Consumer),
            quote!(::nestrs_kafka::Consumer),
        ] {
            assert_eq!(rewritten(untouched.clone()), untouched.to_string());
            assert!(roots_seen(untouched).is_empty());
        }
    }

    #[test]
    fn roots_a_family_member_at_its_family() {
        for (sibling, umbrella) in [
            (
                quote!(::nest_rs_oauth_client::OAuthClient),
                quote!(::nest_rs::oauth::client::OAuthClient),
            ),
            (
                quote!(#[serde(crate = "::nest_rs_oauth_server::serde")]),
                quote!(#[serde(crate = "::nest_rs::oauth::server::serde")]),
            ),
            (
                quote!(::nest_rs_oauth_resource::Resource),
                quote!(::nest_rs::oauth::resource::Resource),
            ),
        ] {
            assert_eq!(rewritten(sibling), umbrella.to_string());
        }
    }

    #[test]
    fn the_missing_dependency_names_the_feature_that_ships_the_crate() {
        let message =
            missing_dependency_message(roots_seen(quote!(::nest_rs_seaorm::__private::uuid::Uuid)))
                .expect("this crate's manifest declares no nest-rs-seaorm");
        assert!(message.contains(r#"features = ["seaorm"]"#), "{message}");

        let message =
            missing_dependency_message(roots_seen(quote!(::nest_rs_oauth_client::OAuthClient)))
                .expect("this crate's manifest declares no nest-rs-oauth-client");
        assert!(
            message.contains(r#"features = ["oauth-client"]"#),
            "{message}"
        );
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
        assert!(walk(&trees, Some(&prefix), "::nest_rs", false, &[], &mut found).is_none());
        assert!(found.is_empty());
    }
}
