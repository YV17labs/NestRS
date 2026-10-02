//! The keys join: every datastore key both workspaces spell, against the span
//! target of the crate that owns the concern it belongs to.
//!
//! A key is a name an operator types — in a chart's KEDA trigger, in a `SCAN`
//! pattern during an incident, in a dashboard — so it obeys the naming law like
//! any other name, and gets the derivation the crate, the span target and the
//! `#[config]` namespace already share: `nestrs:<concern>:<structure>`, the
//! concern read off the owning crate's target rather than chosen — or, for the
//! queue, whose members each own several structures, `nestrs:queue:<queue>:<structure>`,
//! a shape the rule has to state before a concern may take it.
//!
//! Two obligations, and the second is why this is a join rather than a
//! paragraph. `SCAN` and `KEYS` match by glob, so `<ns>:queue` beside
//! `<ns>:queue_configs` means the pattern an operator actually types —
//! `<ns>:queue*` — returns the second with the first. That is the hazard
//! `filters` already guards for `EnvFilter`, which compares a directive with
//! `starts_with`; the same shape, one surface over.
//!
//! **A prefix is a level only when the longer name continues with the
//! separator.** `nestrs:queue` ahead of `nestrs:queue:jobs` is a level a reader
//! sees; `nestrs:queue:job` ahead of `nestrs:queue:jobs` is an accident. That
//! single character is the whole test.
//!
//! Members are derived, never listed: every string literal under `crates/` and
//! `demo/` that opens with the framework's prefix.
//!
//! **A key an operator types is not always written in Rust**, and the two
//! obligations above are worth nothing at the site where that is false. A chart's
//! KEDA trigger names a pending list, `NOTES.txt` prints a `LLEN`, a docs page
//! shows a `SCAN` — none of them builds the name from the constant, so none of
//! them moves when the constant does. `every_key_written_outside_rust_is_built_from_one_the_code_declares`
//! is the second test, and the drift it exists for is silent by nature: KEDA
//! polling a list nobody fills reads zero, holds the deployment at
//! `minReplicaCount`, and never says the key it asked for stopped existing.
//!
//! **A key outside the prefix is still a key**, and that is the one family this
//! join could not see by reading for the prefix alone: the queue's lists, which
//! apalis derives from the namespace it is handed — `<namespace>:active` — and
//! which the 6.x layout hands it as the queue's bare name, at the root of the
//! keyspace. [`APALIS_STRUCTURES`] is what lets the join read them, and every one
//! it finds is a hole: no concern, no constant, nothing that moves them.
//!
//! **apalis's structures are reached through apalis's public API, with one
//! written exception, and the exception is held to its file.** The 6.x boot
//! check reads the 6.x layout itself — apalis's calls count no schedule, and its
//! one counting script fails whole on any key of another type — so
//! `apalis_structures_are_named_only_by_the_6x_check` admits a name of apalis's,
//! read off its `Config` getters, in [`LEGACY_CHECK`] and nowhere else, and
//! `the_6x_check_only_reads` admits there a command sent through `redis::cmd`
//! with a literal name from [`LEGACY_READS`] and no other way to Redis at all.
//! Both read tokens rather than expressions, so a call inside a macro body —
//! `format!`, `tokio::try_join!` — is read as one outside it. What they cannot
//! see is a name or a command that reaches the file through another item — a
//! helper in another file returning a `Cmd`, an alias of `cmd` declared
//! elsewhere — which is why the file may spell no `Cmd` and every `cmd` in it
//! must be called with a literal, and the review owes the rest.
//!
//! **What it reads by its spelling** — declared to the `blinds` join in
//! [`followed`] — is the 6.x check's way to Redis (`cmd`, `Cmd`) and the getters
//! of apalis's `Config` it names structures by; the `blinds` join refuses each
//! renamed, aliased or written by a `macro_rules!`. Every other member is a string
//! literal, read in every macro's tokens — a `macro_rules!` transcriber's included.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    declared_targets, each_source, files_with_extension, files_with_name, is_cfg_test, read,
    relative, repo_root,
};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use syn::LitStr;
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    let mut followed = vec![Followed::call("cmd")];
    followed.extend(APALIS_GETTERS.iter().map(|getter| Followed::call(*getter)));
    followed.extend(OTHER_WAYS_TO_REDIS.iter().map(|way| {
        if way.starts_with(char::is_uppercase) {
            Followed::type_(*way)
        } else {
            Followed::call(*way)
        }
    }));
    followed
}

const BASELINE: &str = "keys-baseline.txt";
const OUTSIDE_BASELINE: &str = "keys-outside-baseline.txt";

/// The leading segment every key the framework writes carries. Hard-coded on
/// purpose: `NESTRS_ENV_PREFIX` renames the *developer's* variables, while a
/// datastore key is the framework's own machinery and two deployments sharing a
/// Redis are separated by the logical database in the connection URL.
const PREFIX: &str = "nestrs:";

/// The separator, and the only character that turns a prefix into a level.
const SEP: char = ':';

/// The structures apalis-redis derives from the namespace it is handed:
/// `<namespace>:active`, `<namespace>:inflight:<worker>`, `<namespace>:data`, and
/// the rest — `apalis-redis-0.7.4/src/storage.rs`, the constants from
/// `ACTIVE_JOBS_LIST` to `SIGNAL_LIST`.
///
/// **Stated rather than derived, and it has to be:** the words are a dependency's
/// layout, not this tree's, and nothing here can read another crate's source. The
/// list is pinned to the release the root manifest pins, and
/// [`the_apalis_layout_read_here_is_the_pinned_releases`] fails the day that
/// requirement moves, so the list is re-read with the move rather than trusted
/// past it.
const APALIS_STRUCTURES: [&str; 9] = [
    "active",
    "consumers",
    "data",
    "dead",
    "done",
    "failed",
    "inflight",
    "scheduled",
    "signal",
];

/// The apalis-redis requirement [`APALIS_STRUCTURES`] was read off.
const APALIS_REDIS_PIN: &str = "0.7";

/// What a key may carry. A placeholder (`{queue}`, `<queue>`) and a glob (`*`)
/// are in, because both are how a key that *varies* is written down; whitespace
/// is out, which is the whole of what separates a key from a message opening
/// with the framework's word.
fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | ':' | '.' | '-' | '*' | '#' | '{' | '}' | '<' | '>')
}

/// One spelling for every placeholder, so a key's identity does not move with
/// the *style* of the format string that writes it.
///
/// `format!("nestrs:throttler:buckets:{subject}")` and
/// `format!("nestrs:throttler:buckets:{}", subject)` name one key, and clippy
/// pushes codebases from the second to the first — so recording the identifier
/// made a cosmetic edit rewrite a baseline line, which the rules class as "a
/// regression argued in review". Both read `nestrs:throttler:buckets:{}`.
fn placeholders_collapsed(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut depth = 0usize;
    for c in key.chars() {
        match c {
            '{' | '<' => {
                if depth == 0 {
                    out.push('{');
                }
                depth += 1;
            }
            '}' | '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push('}');
                }
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// How many levels a name carries — `nestrs:queue` is two, `nestrs:queue:jobs`
/// is three.
fn levels(key: &str) -> usize {
    key.split(SEP).count()
}

/// Whether `key` is one of apalis's structures at the root of the keyspace —
/// `<namespace>:<structure>[:…]` whose namespace does not open with the
/// framework's prefix, so no concern and no constant of ours reaches it.
///
/// The namespace is one level: apalis appends its structure to the name it is
/// handed, and a name holding a `:` is the prefix's case, not this one. An
/// empty namespace is prose naming the suffix alone (`` `:active` ``), not a key.
fn at_the_root(key: &str) -> bool {
    if key.starts_with(PREFIX) {
        return false;
    }
    let mut segments = key.split(SEP);
    let (Some(namespace), Some(structure)) = (segments.next(), segments.next()) else {
        return false;
    };
    !namespace.is_empty()
        && APALIS_STRUCTURES.contains(&structure.trim_end_matches('*'))
        && key.chars().all(is_key_char)
}

/// Whether `key` names one of apalis's structures under whatever namespace it
/// was handed — a literal, a placeholder, the framework's prefix — as a
/// derivation written by hand would: any level after the first is one of
/// apalis's words. A sentence is not a key ([`is_key_char`]).
fn names_an_apalis_structure(key: &str) -> bool {
    key.contains(SEP)
        && key.chars().all(is_key_char)
        && key
            .split(SEP)
            .skip(1)
            .any(|level| APALIS_STRUCTURES.contains(&level.trim_end_matches('*')))
}

/// Below this the walk is reading the wrong tree, and a hole it fails to report
/// is a hole nobody looks for again.
const FLOOR: usize = 2;

/// Every key both workspaces spell, each with the files that spell it.
#[derive(Default)]
struct Scan {
    file: String,
    /// Whether [`Scan::file`] is code the framework runs — under a `src/`, and
    /// not a suite of its own.
    file_runs: bool,
    /// How deep the walk is inside a `#[cfg(test)]` or `#[test]` item.
    in_test: usize,
    found: BTreeMap<String, BTreeSet<String>>,
    /// The keys spelled in code the framework runs, which is what a chart, a
    /// page or a namespace exemption may lean on.
    running: BTreeSet<String>,
    /// apalis structures at the root of the keyspace, spelled in Rust.
    rooted: BTreeMap<String, BTreeSet<String>>,
    /// apalis structures the framework's running code derives by hand — any
    /// name, placeholder or prefix, ending in one of apalis's words.
    by_hand: BTreeMap<String, BTreeSet<String>>,
}

impl Scan {
    /// A key, or the namespace a key is built on: both are names in the one
    /// space an operator globs, so both are members.
    ///
    /// **A sentence that opens with the framework's word is not a key**, and
    /// scanning macro bodies is what made that distinction load-bearing:
    /// `"nestrs: installing dev toolchain…"` is a console line, and three of them
    /// read as keys naming no concern. [`is_key_char`] decides it, and what it
    /// rules out is whitespace — a key never carries any, at any site, while a
    /// message almost always does.
    ///
    /// **It does not rule out a control character.** `queue_name.rs` refuses one
    /// in a queue *name*, but a key's member is not always a name:
    /// `nest_rs_throttler`'s bucket subject joins its parts with `U+001F` and
    /// carries a route pattern, so `nestrs:throttler:buckets:http␟/users/:id␟…`
    /// is a live key of this framework. Nothing spells one as a literal, so this
    /// is the charset staying honest about what it can see rather than a hole.
    ///
    /// A literal opening with a placeholder is left alone: in Rust that is how a
    /// key is built on a constant (`format!("{NAMESPACE}:{name}")`), so what it
    /// names cannot be read from the literal, and the rule asks for the constant.
    fn take(&mut self, value: String) {
        if self.file_runs
            && self.in_test == 0
            && self.file.starts_with("crates/")
            && names_an_apalis_structure(&value)
        {
            self.by_hand
                .entry(value.clone())
                .or_default()
                .insert(self.file.clone());
        }
        let ours = value
            .strip_prefix(PREFIX)
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(is_key_char));
        if ours {
            let key = placeholders_collapsed(&value);
            if self.file_runs && self.in_test == 0 {
                self.running.insert(key.clone());
            }
            self.found.entry(key).or_default().insert(self.file.clone());
        } else if !value.starts_with(['{', '<']) && at_the_root(&value) {
            self.rooted
                .entry(placeholders_collapsed(&value))
                .or_default()
                .insert(self.file.clone());
        }
    }

    fn within(&mut self, is_test: bool, visit: impl FnOnce(&mut Self)) {
        self.in_test += usize::from(is_test);
        visit(self);
        self.in_test -= usize::from(is_test);
    }
}

/// Whether an item is test code: `#[cfg(test)]`, or a test function under any
/// runner's attribute (`#[test]`, `#[tokio::test]`).
fn is_test(attrs: &[syn::Attribute]) -> bool {
    is_cfg_test(attrs)
        || attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "test")
        })
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_lit_str(&mut self, node: &'ast LitStr) {
        self.take(node.value());
    }

    /// **A macro body is where a key is actually assembled, and `syn` does not
    /// parse it.** `format!("nestrs:throttler:buckets:{subject}")` holds a string
    /// literal that never becomes a `LitStr` node, so a walk over items alone
    /// reads the constants and misses every name built beside them. The tokens
    /// are scanned as tokens: a literal is read for its value, and a prefix
    /// reached only through a `{CONST}` interpolation is still invisible, which
    /// is why the rule asks for the constant rather than the format string.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        for literal in nest_rs_conformance::sources::string_literals(node.tokens.clone()) {
            self.take(literal);
        }
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        self.within(is_test(&node.attrs), |scan| {
            syn::visit::visit_item_mod(scan, node);
        });
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.within(is_test(&node.attrs), |scan| {
            syn::visit::visit_item_fn(scan, node);
        });
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.within(is_test(&node.attrs), |scan| {
            syn::visit::visit_item_impl(scan, node);
        });
    }
}

/// The concern each declared span target names, as a key spells it:
/// `nest_rs::queue` → `queue`, `nest_rs::oauth::client` → `oauth:client`.
fn concerns() -> BTreeSet<String> {
    declared_targets()
        .iter()
        .filter_map(|(target, _, _)| target.strip_prefix("nest_rs::"))
        .map(|concern| concern.replace("::", ":"))
        .collect()
}

/// Whether `key`'s segments after the prefix open with one of the concerns —
/// the whole concern, ending at a separator or at the key's end, so `queue`
/// never answers for `queue_configs`.
fn names_a_concern(key: &str, concerns: &BTreeSet<String>) -> bool {
    let Some(rest) = key.strip_prefix(PREFIX) else {
        return false;
    };
    concerns.iter().any(|concern| {
        rest.strip_prefix(concern.as_str())
            .is_some_and(|after| after.is_empty() || after.starts_with(SEP))
    })
}

/// Whether a directive naming `outer` also reaches `inner` without a level
/// between them — `starts_with`, the way a glob matches, minus the one case a
/// reader can see.
fn swallows(outer: &str, inner: &str) -> bool {
    outer != inner
        && inner
            .strip_prefix(outer)
            .is_some_and(|after| !after.starts_with(SEP))
}

fn spelled(files: &BTreeSet<String>) -> String {
    files.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// The sentence for a key at the root of the keyspace, wherever it is spelled.
fn rooted_hole(key: &str, files: &BTreeSet<String>) -> String {
    format!(
        "`{key}` is apalis's layout at the root of the keyspace — its namespace does \
         not open with `{PREFIX}`, so it names no concern, no constant reaches it, and \
         a scan of the framework's keys misses it ({})",
        spelled(files),
    )
}

/// Every key both workspaces spell **in Rust**, which is the only place a key can
/// be built from a constant.
fn declared_in_rust(root: &Path) -> Scan {
    let mut scan = Scan::default();
    each_source(root, |rel, ast| {
        scan.file = rel.to_owned();
        scan.file_runs = rel.contains("/src/") && !rel.contains("/tests/");
        scan.in_test = 0;
        scan.visit_file(ast);
    });
    scan
}

/// The files an operator or a reader takes a key from without the compiler in
/// between — a chart and its schema, its notes, a script, a recipe, a page.
///
/// `json` and `sh` and `just` are here because the sites are real: a chart's
/// `values.schema.json` is where a `default` for a `listName` would sit, a
/// dashboard is JSON, and `demo/`'s scripts and recipes run `redis-cli` by hand.
const OUTSIDE: [&str; 9] = [
    "yaml", "yml", "tpl", "txt", "md", "mdx", "json", "sh", "just",
];

/// Files an operator reads that carry no extension at all.
const OUTSIDE_NAMED: [&str; 2] = ["Justfile", "Dockerfile"];

/// What each walked root owes, so a root that moves or is renamed fails instead
/// of contributing nothing.
///
/// **One floor over the whole walk could not see `demo/` disappear**: pointing
/// the walk at a missing directory returned zero keys from it and the total,
/// carried by `docs/` alone, still cleared the bar. `files_with_extension`
/// answers empty for a path that is not there, so the guard has to be per root
/// or it is not a guard. `demo/`'s floor is its chart's two KEDA triggers.
const OUTSIDE_ROOTS: [(&str, usize); 2] = [("demo", 2), ("docs/src/content", 1)];

/// A key as a name, with the parts that stand for *any* member removed.
///
/// An operator's `SCAN nestrs:throttler:buckets:*` names a pattern over members,
/// not a key, so the member is not what moves when a constant does — and reading
/// the glob as a segment reported a correct `SCAN` pattern as a drift. A
/// trailing wildcard or placeholder is dropped; one in the middle stays, because
/// a key's *structure* is never a placeholder.
fn without_a_wildcard_member(key: &str) -> &str {
    key.trim_end_matches("{}")
        .trim_end_matches('*')
        .trim_end_matches(SEP)
}

/// Whether `text[..at]` ends inside a name, so a match at `at` would be the tail
/// of one rather than the start of a key. A DSN spells
/// `postgres://nestrs:nestrs@…` and a mount `/workspaces/nestrs:cached`, neither
/// of which is a key, so a path separator counts as a name character here.
fn inside_a_name(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Every `nestrs:…` run in `text`, truncated where a placeholder or a glob
/// starts, so `nestrs:throttler:buckets:<subject>` is read as the structure it
/// claims rather than as a key nobody wrote.
fn keys_in(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (at, _) in text.match_indices(PREFIX) {
        if inside_a_name(text, at) {
            continue;
        }
        let rest = &text[at + PREFIX.len()..];
        let end = rest.find(|c: char| !is_key_char(c)).unwrap_or(rest.len());
        let key = format!("{PREFIX}{}", &rest[..end]);
        let key = placeholders_collapsed(key.trim_end_matches([':', '.', '-']));
        if key.len() > PREFIX.len() {
            out.insert(key);
        }
    }
    out
}

/// Every apalis structure at the root of the keyspace that `text` spells —
/// `audio:active` in a KEDA trigger, `<queue>:inflight` in a note, the lines a
/// page prints from `redis-cli KEYS`. Outside Rust a leading placeholder is the
/// member, not a constant: a page writing `<queue>:active` is naming the layout
/// at the root.
fn rooted_in(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for structure in APALIS_STRUCTURES {
        let suffix = format!("{SEP}{structure}");
        for (at, _) in text.match_indices(&suffix) {
            let after = &text[at + suffix.len()..];
            if after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }
            let start = text[..at]
                .char_indices()
                .rev()
                .take_while(|(_, c)| is_key_char(*c))
                .last()
                .map_or(at, |(i, _)| i);
            if inside_a_name(text, start) {
                continue;
            }
            let rest = &text[start..];
            let end = rest.find(|c: char| !is_key_char(c)).unwrap_or(rest.len());
            let key = placeholders_collapsed(rest[..end].trim_end_matches([':', '.', '-']));
            if at_the_root(&key) {
                out.insert(key);
            }
        }
    }
    out
}

/// Whether `key` is still built from something `src/` runs.
///
/// **A namespace is not a key, and reading it as one is what made the first
/// version of this test unable to fail.** Accepting any spelling *below* a
/// declared name exempted the whole subtree under a namespace, so the test
/// caught a renamed **concern** and nothing below it — while the site its own
/// doc names first is a KEDA trigger naming a structure.
///
/// So the relation is to a declared name that reaches the key's **structure**:
/// `nestrs:<concern>:<structure>` is the grammar, so a key of three levels or
/// more has to be built from a declared name of at least three. A spelling of
/// two levels is a concern's namespace rather than a key, and passes when the
/// code declares a key under it.
///
/// What it deliberately does not check is the **member**: a chart naming a queue
/// no `#[queue]` declares is an application's own error, and this join derives
/// keys, not queue names. So a declared name's placeholder level — the `{}` of
/// `nestrs:queue:{}`, where the code writes each queue's name — accepts
/// whatever one level a spelling puts there: `nestrs:queue:audio:active`, the
/// list a KEDA trigger names, is built on it exactly as `nestrs:queue:<queue>:active`
/// in a page is. The level still has to be there, and still has to be one.
fn built_from_a_declared_key(key: &str, declared: &BTreeSet<String>) -> bool {
    if declared.iter().any(|name| same_levels(name, key)) {
        return true;
    }
    if levels(key) <= 2 {
        return declared
            .iter()
            .any(|name| name.starts_with(&format!("{key}{SEP}")));
    }
    declared
        .iter()
        .filter(|name| levels(name) >= 3)
        .any(|name| levels(key) > levels(name) && within(name, key))
}

/// Whether `key` opens with every level of `name`, a `{}` level of `name`
/// standing for any one level.
fn within(name: &str, key: &str) -> bool {
    name.split(SEP)
        .zip(key.split(SEP))
        .all(|(declared, spelled)| declared == "{}" || declared == spelled)
}

/// Whether `key` is `name`, level for level, a `{}` level standing for any one.
fn same_levels(name: &str, key: &str) -> bool {
    levels(name) == levels(key) && within(name, key)
}

#[test]
fn every_key_written_outside_rust_is_built_from_one_the_code_declares() {
    let root: &Path = &repo_root();
    let declared = declared_in_rust(root).running;

    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut rooted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (dir, floor) in OUTSIDE_ROOTS {
        let mut here = 0usize;
        let base = root.join(dir);
        let mut paths: Vec<std::path::PathBuf> = OUTSIDE
            .iter()
            .flat_map(|extension| files_with_extension(&base, extension))
            .collect();
        paths.extend(
            OUTSIDE_NAMED
                .iter()
                .flat_map(|name| files_with_name(&base, name)),
        );
        for path in paths {
            let rel = relative(&path, root);
            let Ok(text) = read(&path) else {
                continue;
            };
            for key in keys_in(&text) {
                here += 1;
                found.entry(key).or_default().insert(rel.clone());
            }
            for key in rooted_in(&text) {
                here += 1;
                rooted.entry(key).or_default().insert(rel.clone());
            }
        }
        baseline::floor(
            here,
            floor,
            &format!("datastore key spelling(s) under {dir}"),
        );
    }

    let mut holes = BTreeSet::new();
    for (key, files) in &found {
        // `nestrs:{}:{}` is the grammar quoted in prose, not a key: a key's
        // concern is a word, so a placeholder there means the sentence is
        // teaching the derivation rather than naming something in a store.
        if key
            .split(SEP)
            .nth(1)
            .is_some_and(|concern| concern.contains("{}"))
        {
            continue;
        }
        if !built_from_a_declared_key(without_a_wildcard_member(key), &declared) {
            holes.insert(format!(
                "`{key}` is written where no constant reaches it — no key `src/` \
                 spells is this name or reaches its structure level, so this \
                 spelling no longer moves when the constant does ({})",
                spelled(files),
            ));
        }
    }
    for (key, files) in &rooted {
        holes.insert(rooted_hole(key, files));
    }

    baseline::gate(
        OUTSIDE_BASELINE,
        &holes,
        found.len() + rooted.len(),
        "datastore key(s) outside Rust",
        "keys",
        "a key an operator reads from a chart, its notes, a script or a page, \
         spelled where nothing moves it when the constant does",
    );
}

#[test]
fn every_key_names_a_concern_and_prefixes_no_other() {
    let root: &Path = &repo_root();
    let scan = declared_in_rust(root);
    baseline::floor(scan.found.len(), FLOOR, "datastore key(s)");

    let concerns = concerns();
    let mut holes = BTreeSet::new();
    for (key, files) in &scan.found {
        if !names_a_concern(key, &concerns) {
            holes.insert(format!(
                "`{key}` names no concern — its second segment is not the tail \
                 of any declared span target ({})",
                spelled(files),
            ));
        }
    }
    // **The structure level, which prose made mandatory and nothing enforced.**
    // `nestrs:<concern>:<structure>[:<member>]`, so a key of two levels is a
    // *namespace* — legitimate only when the code builds keys under it. One with
    // nothing below it is a concern whose keys carry no structure, which is the
    // shape the grammar refuses and the shape the throttler's counters had. Both
    // halves read code the framework runs: a *test* spelling a key under a name
    // exempted the name, so removing the structure level from the code and
    // leaving the e2e's own literal behind passed — and a test's two-level
    // literal is a pattern it scans or a value it refuses, never a key written.
    for key in &scan.running {
        let a_namespace_something_builds_on = scan
            .running
            .iter()
            .any(|other| other.starts_with(&format!("{key}{SEP}")));
        if levels(key) < 3 && !a_namespace_something_builds_on {
            holes.insert(format!(
                "`{key}` carries no structure level — `nestrs:<concern>:<structure>` is the \
                 grammar, and nothing is built under this name, so an operator has no pattern \
                 between the concern and every key in it ({})",
                spelled(&scan.found[key]),
            ));
        }
    }
    for (key, files) in &scan.rooted {
        holes.insert(rooted_hole(key, files));
    }

    let names: Vec<&String> = scan.found.keys().collect();
    for outer in &names {
        for inner in &names {
            if swallows(outer, inner) {
                holes.insert(format!(
                    "`{outer}` swallows `{inner}` — a glob naming the first \
                     reaches the second, with no level between them ({} / {})",
                    spelled(&scan.found[*outer]),
                    spelled(&scan.found[*inner]),
                ));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        scan.found.len() + scan.rooted.len(),
        "datastore key(s)",
        "keys",
        "a key an operator cannot name without naming another, or one whose \
         concern no crate declares",
    );
}

/// [`APALIS_STRUCTURES`] is apalis-redis 0.7's layout, so the requirement it was
/// read off is asserted where the list is used: a move of the pin — which the
/// root manifest says happens only through the resilience suite — fails here, and
/// the list is re-read with it rather than trusted past it.
#[test]
fn the_apalis_layout_read_here_is_the_pinned_releases() {
    let manifest = read(&repo_root().join("Cargo.toml")).expect("the root manifest reads");
    let parsed: toml_edit::DocumentMut = manifest.parse().expect("the root manifest parses");
    let requirement = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("apalis-redis"))
        .and_then(|dependency| {
            dependency.as_str().or_else(|| {
                dependency
                    .get("version")
                    .and_then(|version| version.as_str())
            })
        });
    assert_eq!(
        requirement,
        Some(APALIS_REDIS_PIN),
        "apalis-redis moved off the release APALIS_STRUCTURES was read from — re-read \
         its key layout (`src/storage.rs`) into the list, then move APALIS_REDIS_PIN",
    );
}

/// A declared name's placeholder level stands for whatever member a spelling
/// puts there, and only for that level: every fixed level around it still has
/// to match, so a spelling that renames the concern or the structure is still a
/// hole.
#[test]
fn a_placeholder_level_accepts_any_member_and_nothing_else() {
    let declared = BTreeSet::from([["nestrs", "queue", "{}"].join(":")]);
    let spelled = |segments: &[&str]| segments.join(":");
    for built in [
        spelled(&["nestrs", "queue", "audio"]),
        spelled(&["nestrs", "queue", "audio", "active"]),
        spelled(&["nestrs", "queue", "{}", "inflight", "{}"]),
    ] {
        assert!(built_from_a_declared_key(&built, &declared), "{built}");
    }
    for hole in [
        spelled(&["nestrs", "queues", "audio", "active"]),
        spelled(&["nestrs", "throttler", "audio"]),
    ] {
        assert!(!built_from_a_declared_key(&hole, &declared), "{hole}");
    }
}

/// **apalis's structures are apalis's**, and the framework reads them only
/// through apalis's public API (`framework.md`, *A key a datastore holds*). A
/// name spelled by hand — `{queue}:active`, `{}:inflight:{}` — is a second copy
/// of apalis's derivation: were apalis to move it, the copy would find nothing
/// and say nothing. Every name the framework's running code needs is read off
/// `apalis_redis::Config`'s getters, so no literal under a `crates/*/src/`
/// spells one; a suite may, since it states the layout it checks.
#[test]
fn no_framework_code_spells_an_apalis_structure_by_hand() {
    let scan = declared_in_rust(&repo_root());
    let holes: Vec<String> = scan
        .by_hand
        .iter()
        .map(|(key, files)| {
            format!(
                "`{key}` spells apalis-redis's key derivation by hand ({}): read it off \
                 `apalis_redis::Config`'s getters, so the name moves when apalis's does",
                spelled(files),
            )
        })
        .collect();
    assert!(holes.is_empty(), "{}", holes.join("\n"));
}

/// What counts as a hand-spelled apalis name: any namespace, then one of
/// apalis's words at any level past the first — and nothing that only shares
/// the separator or the word.
#[test]
fn a_hand_spelled_apalis_name_is_one_of_its_words_under_any_namespace() {
    let key = |segments: &[&str]| segments.join(":");
    for spelled in [
        key(&["{queue}", "active"]),
        key(&["{queue}", "inflight", "{queue}"]),
        key(&["{}", "scheduled"]),
        key(&["nestrs", "queue", "{queue}", "consumers"]),
    ] {
        assert!(names_an_apalis_structure(&spelled), "{spelled}");
    }
    for not_one in [
        key(&["active"]),
        key(&["nestrs", "queue", "{queue}"]),
        key(&["nestrs", "queue", "{queue}", "settled", "{job}"]),
        "the queue: active jobs".to_owned(),
        key(&["{queue}", "activeness"]),
    ] {
        assert!(!names_an_apalis_structure(&not_one), "{not_one}");
    }
}

// --- the one exception: the 6.x check reads apalis's structures itself -------------

/// The one file whose running code reads apalis's structures by itself: the
/// 6.x boot check, the written exception to "apalis's structures are apalis's"
/// (`CLAUDE.md`'s hard "no" list, and `framework.md`'s *A key a datastore
/// holds*).
const LEGACY_CHECK: &str = "crates/nest-rs-redis/src/legacy_layout.rs";

/// The getters `apalis_redis::Config` names its structures by — one per
/// structure of [`APALIS_STRUCTURES`], so the only way a name of one reaches
/// running code once a literal is refused. Pinned with that list.
const APALIS_GETTERS: [&str; 9] = [
    "active_jobs_list",
    "consumers_set",
    "dead_jobs_set",
    "done_jobs_set",
    "failed_jobs_set",
    "inflight_jobs_set",
    "job_data_hash",
    "scheduled_jobs_set",
    "signal_list",
];

/// The commands the 6.x check may send: `TYPE`, then the counting read of the
/// type it found. None writes.
const LEGACY_READS: [&str; 5] = ["TYPE", "LLEN", "ZCARD", "ZRANGE", "SCARD"];

/// Every way `redis` 0.32 reaches Redis other than `redis::cmd(<name>)`: a
/// script, a pipeline, a command object built by a constructor or a typed
/// method of `Cmd` or `Pipeline`, a typed command method on a connection —
/// which needs one of these traits in scope, or spelled in a qualified call —
/// and a packed command sent by hand. Read off the release [`REDIS_PIN`] names.
const OTHER_WAYS_TO_REDIS: [&str; 14] = [
    "Cmd",
    "Script",
    "ScriptInvocation",
    "Pipeline",
    "pipe",
    "Commands",
    "AsyncCommands",
    "TypedCommands",
    "AsyncTypedCommands",
    "invoke_async",
    "req_packed_command",
    "req_packed_commands",
    "send_packed_command",
    "send_packed_commands",
];

/// The `redis` requirement [`OTHER_WAYS_TO_REDIS`] was read off.
const REDIS_PIN: &str = "0.32";

/// What one stretch of running code does with apalis's structures and with
/// Redis.
#[derive(Debug, Default)]
struct RedisReach {
    /// The `Config` getters it names.
    getters: BTreeSet<String>,
    /// Every command it sends through `cmd`, or `None` for a `cmd` whose
    /// command is not one literal — a variable, or a `cmd` not called at all,
    /// which is how an alias of it is declared.
    commands: Vec<Option<String>>,
    /// Every other way to Redis it spells.
    other_ways: BTreeSet<String>,
}

impl RedisReach {
    /// Read `tokens` at every depth, each identifier against the token that
    /// follows it at its own level — so a call inside a macro body is read as
    /// one outside it.
    fn read(&mut self, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for (at, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Group(group) => self.read(group.stream()),
                TokenTree::Ident(ident) => {
                    let name = ident.to_string();
                    if APALIS_GETTERS.contains(&name.as_str()) {
                        self.getters.insert(name.clone());
                    }
                    if OTHER_WAYS_TO_REDIS.contains(&name.as_str()) {
                        self.other_ways.insert(name.clone());
                    }
                    if name == "cmd" {
                        self.commands.push(called_with_a_literal(trees.get(at + 1)));
                    }
                }
                _ => {}
            }
        }
    }
}

/// The literal a call's parentheses hold when they hold one literal and
/// nothing else.
fn called_with_a_literal(next: Option<&TokenTree>) -> Option<String> {
    let Some(TokenTree::Group(group)) = next else {
        return None;
    };
    if group.delimiter() != Delimiter::Parenthesis {
        return None;
    }
    let inside: Vec<TokenTree> = group.stream().into_iter().collect();
    match inside.as_slice() {
        [TokenTree::Literal(literal)] => syn::parse_str::<LitStr>(&literal.to_string())
            .ok()
            .map(|literal| literal.value()),
        _ => None,
    }
}

/// The items of `file` its build runs — every item but a `#[cfg(test)]` one or
/// a test function, a module's own items read the same way.
fn running_items(items: &[syn::Item], out: &mut Vec<TokenStream>) {
    for item in items {
        let attrs = match item {
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            syn::Item::Struct(item) => &item.attrs,
            syn::Item::Enum(item) => &item.attrs,
            syn::Item::Trait(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            syn::Item::Macro(item) => &item.attrs,
            _ => {
                out.push(item.to_token_stream());
                continue;
            }
        };
        if is_test(attrs) {
            continue;
        }
        match item {
            syn::Item::Mod(module) => {
                if let Some((_, inner)) = &module.content {
                    running_items(inner, out);
                }
            }
            other => out.push(other.to_token_stream()),
        }
    }
}

/// What each running source of both workspaces reaches, by file.
fn reach_by_file(root: &Path) -> BTreeMap<String, RedisReach> {
    let mut by_file = BTreeMap::new();
    each_source(root, |rel, ast| {
        if !rel.contains("/src/") || rel.contains("/tests/") {
            return;
        }
        let mut running = Vec::new();
        running_items(&ast.items, &mut running);
        let mut reach = RedisReach::default();
        for tokens in running {
            reach.read(tokens);
        }
        by_file.insert(rel.to_owned(), reach);
    });
    by_file
}

/// A name of apalis's reaches the running code of both workspaces in one file:
/// the 6.x check. Anywhere else it is a read or a write of apalis's structures
/// around apalis's API — the rule's whole point — or a copy of a name that
/// moves when apalis's derivation does.
#[test]
fn apalis_structures_are_named_only_by_the_6x_check() {
    let by_file = reach_by_file(&repo_root());
    let check = by_file
        .get(LEGACY_CHECK)
        .unwrap_or_else(|| panic!("{LEGACY_CHECK} is the 6.x check; the walk did not find it"));
    assert!(
        !check.getters.is_empty(),
        "{LEGACY_CHECK} names none of apalis's structures — the walk is reading the wrong file",
    );
    let holes: Vec<String> = by_file
        .iter()
        .filter(|(file, reach)| file.as_str() != LEGACY_CHECK && !reach.getters.is_empty())
        .map(|(file, reach)| {
            format!(
                "{file} names apalis's {} — apalis's structures are reached through its public \
                 API, and the one file that reads them itself is {LEGACY_CHECK}",
                reach.getters.iter().cloned().collect::<Vec<_>>().join(", "),
            )
        })
        .collect();
    assert!(holes.is_empty(), "{}", holes.join("\n"));
}

/// The 6.x check only reads: every command it sends goes through `redis::cmd`
/// with a literal name from [`LEGACY_READS`], and it spells no other way to
/// Redis — no script, no pipeline, no command object, no typed method.
#[test]
fn the_6x_check_only_reads() {
    let by_file = reach_by_file(&repo_root());
    let check = by_file
        .get(LEGACY_CHECK)
        .unwrap_or_else(|| panic!("{LEGACY_CHECK} is the 6.x check; the walk did not find it"));
    assert!(
        !check.commands.is_empty(),
        "{LEGACY_CHECK} sends no command — the walk is reading the wrong file",
    );
    let mut holes = Vec::new();
    for command in &check.commands {
        match command {
            Some(name) if LEGACY_READS.contains(&name.as_str()) => {}
            Some(name) => holes.push(format!(
                "{LEGACY_CHECK} sends `{name}`; the 6.x check sends {} and nothing else",
                LEGACY_READS.join(", "),
            )),
            None => holes.push(format!(
                "{LEGACY_CHECK} spells a `cmd` that is not called with one literal name — a \
                 command the join cannot read, or an alias of `cmd`",
            )),
        }
    }
    for other in &check.other_ways {
        holes.push(format!(
            "{LEGACY_CHECK} spells `{other}`, a way to Redis other than `redis::cmd` with a read's \
             literal name",
        ));
    }
    assert!(holes.is_empty(), "{}", holes.join("\n"));
}

/// The reader sees a call inside a macro body as one outside it, reads a
/// command only when it is one literal, and names every other way to Redis.
#[test]
fn the_reach_of_code_is_read_through_macros_and_refuses_what_it_cannot_read() {
    let reach = |code: &str| {
        let mut reach = RedisReach::default();
        reach.read(code.parse().expect("tokens"));
        reach
    };
    let read = reach(r#"redis::cmd("TYPE").arg(&key).query_async(&mut conn).await"#);
    assert_eq!(read.commands, [Some("TYPE".to_owned())]);
    assert!(read.other_ways.is_empty());

    let in_a_macro = reach(r#"tokio::try_join!(redis::cmd("DEL").arg(&key).query_async(&mut c))"#);
    assert_eq!(in_a_macro.commands, [Some("DEL".to_owned())]);

    for unreadable in ["redis::cmd(name).arg(&key)", "use redis::cmd as read;"] {
        assert_eq!(reach(unreadable).commands, [None], "{unreadable}");
    }
    for (code, way) in [
        ("redis::pipe().del(&key)", "pipe"),
        ("Script::new(TEXT).invoke_async(&mut conn)", "Script"),
        ("redis::Cmd::del(&key)", "Cmd"),
        ("use redis::AsyncCommands;", "AsyncCommands"),
    ] {
        assert!(reach(code).other_ways.contains(way), "{code}");
    }
    let named = reach(r#"format!("{}", layout.consumers_set())"#);
    assert_eq!(
        named.getters.into_iter().collect::<Vec<_>>(),
        ["consumers_set"]
    );
}

/// [`OTHER_WAYS_TO_REDIS`] is `redis` 0.32's surface, so the requirement it was
/// read off is asserted where the list is used, as [`APALIS_STRUCTURES`]'s is.
#[test]
fn the_redis_surface_read_here_is_the_pinned_releases() {
    let manifest = read(&repo_root().join("Cargo.toml")).expect("the root manifest reads");
    let parsed: toml_edit::DocumentMut = manifest.parse().expect("the root manifest parses");
    let requirement = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("redis"))
        .and_then(|dependency| {
            dependency.as_str().or_else(|| {
                dependency
                    .get("version")
                    .and_then(|version| version.as_str())
            })
        });
    assert_eq!(
        requirement,
        Some(REDIS_PIN),
        "redis moved off the release OTHER_WAYS_TO_REDIS was read from — re-read how it \
         reaches Redis (`Cmd`, `Pipeline`, `Script`, the command traits, `ConnectionLike`) \
         into the list, then move REDIS_PIN",
    );
}

/// The recogniser reads apalis's root layout wherever a page or a chart writes
/// it, and nothing else that shares the separator.
///
/// The expected keys are joined from their segments: spelled whole, this test's
/// own literals would be keys at the root of the keyspace to the scan above.
#[test]
fn a_root_structure_is_read_where_it_is_written_and_nowhere_else() {
    let key = |segments: &[&str]| segments.join(":");
    let text = "listName: audio:active\n\
                `<queue>:inflight` and `<queue>:scheduled`\n\
                $ redis-cli HGET audio:data::result 01K\n\
                pushes straight to `:active`\n\
                postgres://nestrs:nestrs@db/data\n\
                nestrs:queue:audio:active\n\
                audio:activeness http://host:3000/dead";
    assert_eq!(
        rooted_in(text),
        BTreeSet::from([
            key(&["audio", "active"]),
            key(&["audio", "data", "", "result"]),
            key(&["{}", "inflight"]),
            key(&["{}", "scheduled"]),
        ]),
    );
}

/// Where `framework.md` states the datastore-key grammar, including the one
/// shape that puts a member ahead of the structure.
const KEY_RULE: &str = ".claude/rules/framework.md";

/// The level a declared key holds right after its concern, and the concern —
/// `nestrs:queue:{}:open:{}` is `("queue", "{}")`, `nestrs:throttler:buckets` is
/// `("throttler", "buckets")`. The longest declared concern wins, so a family
/// member's key reads its member's concern rather than the family's.
fn after_the_concern<'k>(key: &'k str, concerns: &BTreeSet<String>) -> Option<(String, &'k str)> {
    let rest = key.strip_prefix(PREFIX)?;
    let concern = concerns
        .iter()
        .filter(|concern| {
            rest.strip_prefix(concern.as_str())
                .is_some_and(|after| after.is_empty() || after.starts_with(SEP))
        })
        .max_by_key(|concern| concern.len())?;
    let after = rest[concern.len()..].strip_prefix(SEP)?;
    let level = after.split(SEP).next()?;
    Some((concern.clone(), level))
}

/// Every key the framework writes reads `nestrs:<concern>:<structure>…`, or —
/// for a concern whose members each own several structures — puts the member
/// first, `nestrs:<concern>:<member>:<structure>…`, and the rule has to say
/// which concerns those are.
///
/// **The second shape is the one the grammar left unstated**, and it is every
/// queue key: apalis derives its structures from one namespace per queue, so a
/// reader deriving `nestrs:queue:active:audio` from the three-level grammar
/// names a list nobody fills. So a concern whose running code puts a member
/// third owes a line in [`KEY_RULE`] spelling its shape — `nestrs:<concern>:<`
/// followed on that line by `:<structure>` — and, once it is member-first, every
/// key of it is: one concern, one shape, or a `SCAN` of the concern's member
/// finds half its keys.
#[test]
fn a_member_first_key_is_the_shape_the_rule_states() {
    let root: &Path = &repo_root();
    let concerns = concerns();
    let running = declared_in_rust(root).running;
    let rule = read(&root.join(KEY_RULE)).expect("the key rule reads");

    let mut shapes: BTreeMap<String, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for key in &running {
        let Some((concern, level)) = after_the_concern(key, &concerns) else {
            continue;
        };
        let (member_first, structure_first) = shapes.entry(concern).or_default();
        if level == "{}" {
            member_first.insert(key);
        } else {
            structure_first.insert(key);
        }
    }
    assert!(
        shapes.len() >= FLOOR,
        "found keys under {} concern(s) — the walk is reading the wrong tree",
        shapes.len(),
    );

    let mut holes = Vec::new();
    for (concern, (member_first, structure_first)) in &shapes {
        if member_first.is_empty() {
            continue;
        }
        let opening = format!("{PREFIX}{concern}{SEP}<");
        let stated = rule.lines().any(|line| {
            line.find(&opening)
                .is_some_and(|at| line[at..].contains(":<structure>"))
        });
        if !stated {
            holes.push(format!(
                "`{concern}` puts a member before its structure ({}) and {KEY_RULE} states no \
                 `{opening}…>:<structure>` shape for it",
                member_first.iter().copied().collect::<Vec<_>>().join(", "),
            ));
        }
        if !structure_first.is_empty() {
            holes.push(format!(
                "`{concern}` is member-first, yet {} put(s) a structure where its member goes",
                structure_first
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    }
    assert!(holes.is_empty(), "\n{}", holes.join("\n"));
}
