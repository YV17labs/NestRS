//! Every datastore key written outside Rust is built from one the code declares.
//!
//! A key is a name an operator types — in a chart's KEDA trigger, in a `SCAN`
//! during an incident, in a page — and none of those sites builds it from the
//! constant, so none of them moves when the constant does. The drift is silent
//! by nature: KEDA polling a list nobody fills reads zero, holds the deployment
//! at `minReplicaCount`, and never says the key it asked for stopped existing.
//! That shipped once — the chart's triggers kept the 6.x root-level lists for
//! twelve commits after the keys moved under `nestrs:queue:`.
//!
//! The declared side is the framework's key **constants**: every `const` under
//! a framework crate's `src/` whose value opens with `nestrs:`. What the keys
//! themselves must look like — `nestrs:<concern>:<structure>`, member-first for
//! the queue, no key a non-level prefix of another — is held where the
//! constants live, by a unit test over their values.
//!
//! The spelled side is plain text — charts, notes, scripts, recipes, pages —
//! where no Rust construction can hide a name. Two shapes are read: a
//! `nestrs:…` run, and one of apalis's structures at the root of the keyspace
//! (`audio:active`), which is the 6.x layout and is never a 7.0 key.
//! [`APALIS_STRUCTURES`] is apalis-redis's own list, copied: a structure apalis
//! adds is one this check does not see until it is added here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nest_rs_conformance::sources::{
    files_with_extension, files_with_name, is_cfg_test, parsed, read, relative, repo_root,
    rust_files,
};
use syn::{Expr, Item, Lit};

/// The leading segment every key the framework writes carries.
const PREFIX: &str = "nestrs:";

/// The separator, and the only character that turns a prefix into a level.
const SEP: char = ':';

/// The structures apalis-redis derives from the namespace it is handed —
/// `<namespace>:active`, `<namespace>:inflight:<worker>`, … — as its
/// `storage.rs` names them, from `ACTIVE_JOBS_LIST` to `SIGNAL_LIST`.
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

/// The framework declares more key constants than this; below it the walk is
/// reading the wrong tree.
const DECLARED_FLOOR: usize = 10;

/// The files an operator or a reader takes a key from without the compiler in
/// between — a chart and its schema, its notes, a script, a recipe, a page.
const OUTSIDE: [&str; 9] = [
    "yaml", "yml", "tpl", "txt", "md", "mdx", "json", "sh", "just",
];

/// Files an operator reads that carry no extension at all.
const OUTSIDE_NAMED: [&str; 2] = ["Justfile", "Dockerfile"];

/// What each walked root owes, so a root that moves or is renamed fails instead
/// of contributing nothing. `demo/`'s floor is its chart's two KEDA triggers.
const OUTSIDE_ROOTS: [(&str, usize); 2] = [("demo", 2), ("docs/src/content", 1)];

/// What a key may carry. A placeholder (`{queue}`, `<queue>`) and a glob (`*`)
/// are in, because both are how a key that varies is written down; whitespace
/// is out, which is what separates a key from a sentence opening with the word.
fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | ':' | '.' | '-' | '*' | '#' | '{' | '}' | '<' | '>')
}

/// One spelling for every placeholder — `{queue}`, `{}` and `<queue>` all read
/// `{}` — so a key's identity does not move with how its member is written.
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

/// How many levels a name carries — `nestrs:queue` is two.
fn levels(key: &str) -> usize {
    key.split(SEP).count()
}

/// Every key constant a framework crate declares, placeholders collapsed.
fn declared_keys(root: &Path) -> BTreeSet<String> {
    fn collect(items: &[Item], out: &mut BTreeSet<String>) {
        for item in items {
            match item {
                Item::Const(konst) => {
                    if let Expr::Lit(lit) = &*konst.expr
                        && let Lit::Str(text) = &lit.lit
                        && text.value().starts_with(PREFIX)
                    {
                        out.insert(placeholders_collapsed(&text.value()));
                    }
                }
                Item::Mod(module) if !is_cfg_test(&module.attrs) => {
                    if let Some((_, inner)) = &module.content {
                        collect(inner, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    for file in rust_files(&root.join("crates"))
        .into_iter()
        .filter(|file| relative(file, root).split('/').nth(2) == Some("src"))
    {
        if let Some(ast) = parsed(&file) {
            collect(&ast.items, &mut out);
        }
    }
    out
}

/// Whether `key` is one of apalis's structures at the root of the keyspace —
/// `<namespace>:<structure>[:…]` whose namespace does not open with the prefix.
/// An empty namespace is prose naming the suffix alone (`` `:active` ``).
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

/// A key as a name, with a trailing member — `*` or a placeholder — removed:
/// `SCAN nestrs:throttler:buckets:*` names a pattern over members, and the
/// member is not what moves when a constant does.
fn without_a_wildcard_member(key: &str) -> &str {
    key.trim_end_matches("{}")
        .trim_end_matches('*')
        .trim_end_matches(SEP)
}

/// Whether `text[..at]` ends inside a name, so a match at `at` is the tail of
/// one — `postgres://nestrs:nestrs@…`, `/workspaces/nestrs:cached` — rather
/// than the start of a key.
fn inside_a_name(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Every `nestrs:…` run in `text`, placeholders collapsed.
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
/// `audio:active` in a trigger, `<queue>:inflight` in a note.
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

/// Whether `key` is still built from a declared constant.
///
/// A key of three levels or more must reach the **structure** of a declared key
/// of at least three — reading any spelling below a declared *namespace* as
/// built from it would exempt the whole subtree, and a renamed structure is the
/// drift a KEDA trigger suffers. A spelling of two levels is a concern's
/// namespace and passes when some declared key sits under it. A declared `{}`
/// level stands for any one member: `nestrs:queue:audio:active` is built on
/// `nestrs:queue:{}`.
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

/// Whether `key` is `name`, level for level.
fn same_levels(name: &str, key: &str) -> bool {
    levels(name) == levels(key) && within(name, key)
}

fn spelled(files: &BTreeSet<String>) -> String {
    files.iter().cloned().collect::<Vec<_>>().join(", ")
}

#[test]
fn every_key_written_outside_rust_is_built_from_one_the_code_declares() {
    let root: &Path = &repo_root();
    let declared = declared_keys(root);
    assert!(
        declared.len() >= DECLARED_FLOOR,
        "read {} declared key constant(s), expected at least {DECLARED_FLOOR}",
        declared.len(),
    );

    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut rooted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (dir, floor) in OUTSIDE_ROOTS {
        let mut here = 0usize;
        let base = root.join(dir);
        let mut paths: Vec<PathBuf> = OUTSIDE
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
        assert!(
            here >= floor,
            "read {here} key spelling(s) under {dir}, expected at least {floor}",
        );
    }

    let mut holes = Vec::new();
    for (key, files) in &found {
        // `nestrs:{}:{}` is the grammar quoted in prose, not a key: a concern is
        // a word, so a placeholder there is a sentence teaching the derivation.
        if key
            .split(SEP)
            .nth(1)
            .is_some_and(|concern| concern.contains("{}"))
        {
            continue;
        }
        if !built_from_a_declared_key(without_a_wildcard_member(key), &declared) {
            holes.push(format!(
                "`{key}` is written where no constant reaches it — no key constant \
                 is this name or reaches its structure level ({})",
                spelled(files),
            ));
        }
    }
    for (key, files) in &rooted {
        holes.push(format!(
            "`{key}` is apalis's 6.x layout at the root of the keyspace — 7.0 files \
             every queue structure under `{PREFIX}queue:` ({})",
            spelled(files),
        ));
    }
    assert!(
        holes.is_empty(),
        "{} datastore key(s) spelled outside Rust where nothing moves them when \
         the constant does:\n  {}",
        holes.len(),
        holes.join("\n  "),
    );
}
