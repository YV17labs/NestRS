//! The docs join: every member of a framework family the corpus owes a mention.
//!
//! **The docs were the one population nothing joined.** `edges.rs` carries
//! thirteen columns and not one of them is a docs column; `umbrella.rs` holds
//! the whole documented surface visible from Rust in a single cell, `## Install`.
//! So a family could grow a member, be covered by tests, ship — and be nameable
//! nowhere on the site, with everything green. Two units of work reached 5.1
//! that way: `graphql.operation` and `events.dispatch` appear on **zero** of the
//! 125 pages, while the table that publishes the canonical names lists eight of
//! ten and its shape tells a reader there are no others.
//!
//! This is the general answer rather than three more checks. A family declared
//! in Rust now owes a docs cell **by construction**: add a unit, a target or a
//! config key and this join reports it the day it exists, without anyone
//! remembering that the docs are a thing.
//!
//! **Three families, and the qualifier is the spelling.**
//! `.claude/rules/testing.md` clause 4: "a family whose members cannot be
//! spelled cannot be joined". A unit is `graphql.operation`, a target
//! `nest_rs::http`, an env key `<PREFIX>_WS__MAX_MESSAGE_BYTES` — each a
//! distinctive literal a page either contains or does not. Capabilities are
//! deliberately absent: a feature is spelled `authn`, which is also an English
//! word and a module name, so a grep for it answers a different question. Their
//! documented-install cell lives in `umbrella.rs`, where the spelling is
//! `cargo add nest-rs --features authn` and is distinctive.
//!
//! **The docs are prose, and that is why this join reads them for a *literal*.**
//! `env_names.rs` exempts pages from the no-literal rule — "a docs page is data
//! an operator reads, not code the framework interprets" — which is right, and
//! is exactly what makes them joinable here: the page must spell the name, so
//! the name is what this looks for.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, declared_str, declared_targets, declared_units, files_with_extension, parsed, read,
    relative, repo_root, rust_files,
};
use syn::visit::Visit;

const BASELINE: &str = "docs-baseline.txt";

/// Where the pages are. One root, so a moved corpus fails the floor rather than
/// reporting every member as undocumented.
const CORPUS: &str = "docs/src/content/docs";

/// Below this the corpus walk is reading the wrong tree, and every hole it
/// reports is an artefact. 125 pages stand today.
const PAGE_FLOOR: usize = 100;

/// One floor per family, never one shared.
///
/// A floor belongs to the population it guards — `baseline::floor` argues the
/// *sentence* is central and the number is not, and `umbrella.rs` carried a
/// README corpus behind a capability count until that was pulled apart. Sharing
/// one here would mean a family shrinking to nothing while another's size held
/// the check up.
mod floors {
    pub const UNITS: usize = 8;
    pub const TARGETS: usize = 20;
    pub const ENV_KEYS: usize = 40;
}

/// The prefix a framework variable carries by default, **read from its
/// declaration**.
///
/// `env_names.rs` spells the prefix and exempts its own file by path, arguing
/// that "a scan that cannot see itself is the honest shape". That argument
/// holds, and it is not needed here: `EnvPrefix::DEFAULT` is a `pub const &str`
/// at a known path, which is the shape `declared_str` exists for. Deriving it
/// costs one read and buys the property the whole crate is built on — the day
/// the default changes, this join follows rather than reporting every page as
/// undocumented.
fn default_prefix(root: &Path) -> String {
    let declaration = root.join("crates/nest-rs-core/src/env_prefix.rs");
    let prefix = declared_str(&declaration, "DEFAULT")
        .expect("`EnvPrefix::DEFAULT` declares the prefix a deployment gets without setting one");
    format!("{prefix}_")
}

/// The two targets nothing on the site names, and the reason it is right.
///
/// A span target is a filter directive an operator types, so the site owes one a
/// mention — except where the concern is not the operator's. `nest_rs::container`
/// and `nest_rs::loader` instrument the DI graph and the GraphQL dataloader's
/// internals: a reader has no decision to make about either, and documenting a
/// directive nobody would set is noise the other twenty-four have to compete
/// with. Refused in a sentence rather than baselined, because a baseline line
/// says "not yet" and this is "no".
const INTERNAL_TARGETS: [&str; 2] = ["nest_rs::container", "nest_rs::loader"];

#[test]
fn every_family_member_the_framework_declares_is_named_by_some_page() {
    let root = repo_root();
    let corpus = corpus(&root);
    baseline::floor(corpus.len(), PAGE_FLOOR, "docs pages");

    let mut holes = BTreeSet::new();
    let names = |name: &str| corpus.iter().any(|page| page.contains(name));

    // A unit of work is `<edge>.<unit>`, read by the span, the operation line's
    // `name:` and its message alike. A reader building a dashboard groups on it,
    // so a unit the site never prints is one they cannot query for.
    let units = declared_units();
    baseline::floor(units.len(), floors::UNITS, "declared units of work");
    for (unit, _, _) in units {
        if !names(&unit) {
            holes.insert(format!("unit :: {unit}"));
        }
    }

    // A span target is what `<PREFIX>_LOG` selects on.
    let targets: Vec<&str> = declared_targets()
        .iter()
        .map(|(target, _, _)| *target)
        .filter(|target| !INTERNAL_TARGETS.contains(target))
        .collect();
    baseline::floor(
        targets.len(),
        floors::TARGETS,
        "operator-facing span targets",
    );
    for target in targets {
        if !names(target) {
            holes.insert(format!("target :: {target}"));
        }
    }

    // An env key is the deployment's only handle on a config field, and a page
    // may present it either way — see [`documents_key`].
    let keys = env_keys(&root);
    baseline::floor(keys.len(), floors::ENV_KEYS, "config env keys");
    for key in keys {
        if !corpus.iter().any(|page| documents_key(page, &key)) {
            holes.insert(format!("env :: {}{}", key.namespace_prefix, key.key));
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        corpus.len(),
        "pages",
        "the docs corpus",
        "a member the framework declares and no page names",
    );
}

/// A config variable, in the two halves a page may print separately.
struct EnvKey {
    /// `<PREFIX>_WS__` — what a page states once, above a table.
    namespace_prefix: String,
    /// `MAX_MESSAGE_BYTES` — what the table's rows carry.
    key: String,
}

/// Whether one page documents a variable, in either of the two shapes a page
/// legitimately uses.
///
/// **Spelling the whole name per row is not the convention, and demanding it
/// would be the check inventing one.** `/storage/` states the namespace once and
/// tables the bare keys, which is how an operator reads a config section; the
/// full `<PREFIX>_STORAGE__ACCESS_KEY` appears nowhere and the page is not at
/// fault. So a page documents a key when it prints the whole variable, or when
/// it prints the bare key *and* names the namespace it belongs to. The second
/// half is what stops a bare `REGION` on an unrelated page from counting.
fn documents_key(page: &str, key: &EnvKey) -> bool {
    let whole = format!("{}{}", key.namespace_prefix, key.key);
    page.contains(&whole)
        || (page.contains(&key.namespace_prefix) && page.contains(&format!("`{}`", key.key)))
}

/// Every page's text. Read once: 125 pages against ~80 members is 10 000
/// `contains` calls, and re-reading the tree per member would be 125 times that.
fn corpus(root: &Path) -> Vec<String> {
    let dir = root.join(CORPUS);
    let mut out = Vec::new();
    for extension in ["mdx", "md"] {
        for file in files_with_extension(&dir, extension) {
            if let Ok(text) = read(&file) {
                out.push(text);
            }
        }
    }
    out
}

/// Every `<PREFIX>_<NAMESPACE>__<KEY>` the framework's configs read.
///
/// Derived from the two halves that make one: the `#[config(namespace = "…")]`
/// on the struct, and the key literals its `from_env` hands to `ConfigService`.
/// Neither alone is the variable — which is why the docs' own env-reference page
/// covers five namespaces with "the rule is uniform: the struct's field name
/// uppercased is the key", a sentence that is false for every config whose
/// `from_env` renames a field (`request_timeout` reads `REQUEST_TIMEOUT_SECS`)
/// or flattens a nested one.
fn env_keys(root: &Path) -> Vec<EnvKey> {
    let prefix = default_prefix(root);
    let mut out = Vec::new();
    for dir in crate_dirs() {
        if !dir.starts_with(root.join("crates")) {
            continue;
        }
        for file in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&file) else {
                continue;
            };
            let mut scan = ConfigScan::default();
            scan.visit_file(&ast);
            let Some(namespace) = scan.namespace else {
                continue;
            };
            // The witness crate's own config documents nothing: it exists to
            // prove a decorator needs no second manifest line.
            if relative(&file, root).contains("nest-rs-macro-hygiene") {
                continue;
            }
            let namespace_prefix = format!("{prefix}{}__", namespace.to_uppercase());
            out.extend(scan.keys.into_iter().map(|key| EnvKey {
                namespace_prefix: namespace_prefix.clone(),
                key,
            }));
        }
    }
    out.sort_by(|a, b| (&a.namespace_prefix, &a.key).cmp(&(&b.namespace_prefix, &b.key)));
    out.dedup_by(|a, b| a.namespace_prefix == b.namespace_prefix && a.key == b.key);
    out
}

/// One config file's namespace and the keys its `from_env` reads.
#[derive(Default)]
struct ConfigScan {
    namespace: Option<String>,
    keys: Vec<String>,
}

impl<'ast> Visit<'ast> for ConfigScan {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if let Some(attr) = node.attrs.iter().find(|a| a.path().is_ident("config")) {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("namespace")
                    && let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    self.namespace = Some(lit.value());
                }
                Ok(())
            });
        }
        syn::visit::visit_item_struct(self, node);
    }

    /// A key is the first string-literal argument of a **read** on `env`.
    ///
    /// Keyed on the receiver rather than on the literal's shape: a default value
    /// is a string literal too, and `SCREAMING_SNAKE` is a convention rather
    /// than a guarantee. `env` is the parameter name the `Config` trait fixes,
    /// so it is the one thing every `from_env` has in common.
    ///
    /// `var_name` is excluded and is the only exclusion: it *renders* a name for
    /// an error message rather than reading one, and `nest-rs-http` hands it the
    /// globs `TLS_*` and `CORS_*` to say "one of this group failed". Counted as
    /// keys, those two asked the docs to publish variables that do not exist.
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method != "var_name"
            && let syn::Expr::Path(path) = &*node.receiver
            && path.path.is_ident("env")
            && let Some(syn::Expr::Lit(lit)) = node.args.first()
            && let syn::Lit::Str(key) = &lit.lit
        {
            self.keys.push(key.value());
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}
