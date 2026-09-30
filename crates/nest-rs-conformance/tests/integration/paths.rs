//! The paths join: every framework path the rules and the docs name resolves.
//!
//! A rule or a page that names `nest_rs_queue::envelope` tells a reader where to
//! look, and a reader who types it gets `E0603` — the module went private in
//! 7.0 and the sentence in `CLAUDE.md` did not follow. Nothing compiles prose,
//! so nothing said so: a path in a paragraph is the one reference in this tree
//! that no compiler, no rustdoc link check and no test ever resolves. This join
//! resolves it.
//!
//! **What is read.** The prose a reader follows *outside* the source: `CLAUDE.md`,
//! the rules under `.claude/rules/`, `docs/STYLE.md`, every docs page, every
//! crate's README. The CHANGELOG is left out on purpose — it names what a release
//! removed, and a removed path is the one path that must not resolve. A comment
//! in the source is left out too: `crate::module` is how code names its own
//! internals to the next person editing it, and it is right there.
//!
//! **What resolves.** A path is `nest_rs_<crate>::<segment>…`, or
//! `nest_rs::<concern>::<segment>…` read through the umbrella's own re-exports.
//! Each segment is walked through the crate's `lib.rs` and the `pub mod` files
//! under it until it names a public item — a type, a trait, a function, a
//! constant, a re-export — past which the rest is a member the parser would need
//! the type's `impl`s to judge, and is taken on trust. A module holding a glob
//! re-export answers for any name. A `nest_rs::<word>` whose word is no umbrella
//! re-export is a span target (`nest_rs::operation`), not a path, and is left to
//! the `targets` join.
//!
//! **A module named as a place is a file.** A rule that says a sentence "is
//! worded once in the codegen crate's `job` module" is pointing at source a
//! reader opens, not a path a caller types — so it names the file
//! (`crates/nest-rs-codegen/src/job.rs`), which never pretends to be importable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use nest_rs_conformance::sources::{files_with_extension, parsed, read, relative, repo_root};
use syn::{Item, UseTree, Visibility};

/// Below this the walk is reading the wrong tree.
const FLOOR: usize = 50;

/// The prose a reader follows outside the source.
fn prose(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.join("CLAUDE.md"), root.join("docs/STYLE.md")];
    files.extend(files_with_extension(&root.join(".claude/rules"), "md"));
    files.extend(files_with_extension(
        &root.join("docs/src/content/docs"),
        "mdx",
    ));
    files.extend(files_with_extension(
        &root.join("docs/src/content/docs"),
        "md",
    ));
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let readme = entry.path().join("README.md");
            if readme.is_file() {
                files.push(readme);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// `nest_rs::<alias>` → the crate it re-exports, read off the umbrella's
/// `lib.rs`: `pub use nest_rs_http as http;` at its root, and a family's members
/// one module down (`oauth::client`).
fn umbrella(root: &Path) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    let ast = parsed(&root.join("crates/nest-rs/src/lib.rs")).expect("the umbrella parses");
    collect_aliases(&ast.items, "", &mut aliases);
    aliases
}

fn collect_aliases(items: &[Item], prefix: &str, out: &mut BTreeMap<String, String>) {
    for item in items {
        match item {
            Item::Use(item) if is_pub(&item.vis) => {
                if let UseTree::Rename(rename) = &item.tree {
                    let target = rename.ident.to_string();
                    if target.starts_with("nest_rs_") {
                        out.insert(format!("{prefix}{}", rename.rename), target);
                    }
                }
            }
            Item::Mod(module) if is_pub(&module.vis) => {
                if let Some((_, inner)) = &module.content {
                    collect_aliases(inner, &format!("{prefix}{}::", module.ident), out);
                }
            }
            _ => {}
        }
    }
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// Every `nest_rs…::…` path `text` spells, as `(crate, segments)` once the
/// umbrella is read through, with the line it sits on.
fn paths_in(text: &str, aliases: &BTreeMap<String, String>) -> Vec<(usize, String, Vec<String>)> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = Vec::new();
    for (number, line) in text.lines().enumerate() {
        for (at, _) in line.match_indices("nest_rs") {
            if line[..at].chars().next_back().is_some_and(ident) {
                continue;
            }
            let rest = &line[at..];
            let head_len = rest.find(|c: char| !ident(c)).unwrap_or(rest.len());
            let head = &rest[..head_len];
            let mut segments = Vec::new();
            let mut tail = &rest[head_len..];
            while let Some(after) = tail.strip_prefix("::") {
                let len = after.find(|c: char| !ident(c)).unwrap_or(after.len());
                if len == 0 {
                    break;
                }
                segments.push(after[..len].to_owned());
                tail = &after[len..];
            }
            if head.ends_with('_') || segments.is_empty() {
                continue;
            }
            let (krate, segments) = if head == "nest_rs" {
                // The longest alias the segments open with: `oauth::client`
                // before `oauth`.
                let found = (1..=segments.len().min(2)).rev().find_map(|n| {
                    aliases
                        .get(&segments[..n].join("::"))
                        .map(|krate| (krate.clone(), segments[n..].to_vec()))
                });
                match found {
                    Some(found) => found,
                    None => continue,
                }
            } else {
                (head.to_owned(), segments)
            };
            if !segments.is_empty() {
                out.push((number + 1, krate, segments));
            }
        }
    }
    out
}

/// One module's public names: what each one is, as far as walking on needs.
#[derive(Default)]
struct Module {
    /// A public item or re-export, past which the walk stops.
    names: BTreeSet<String>,
    /// A public module, walked into: its items when written inline, or the
    /// files that may hold them.
    modules: BTreeMap<String, Child>,
    /// Whether a glob re-export makes any name possible.
    glob: bool,
}

/// Where a public module's items are.
enum Child {
    Inline(Vec<Item>),
    File([PathBuf; 2]),
}

/// Every file the walk parsed, so a crate read for its hundredth path is parsed
/// once.
#[derive(Default)]
struct Parsed(HashMap<PathBuf, Option<Vec<Item>>>);

impl Parsed {
    fn items(&mut self, path: &Path) -> Option<Vec<Item>> {
        self.0
            .entry(path.to_owned())
            .or_insert_with(|| parsed(path).map(|file| file.items))
            .clone()
    }
}

fn leaves(tree: &UseTree, module: &mut Module) {
    match tree {
        UseTree::Path(path) => leaves(&path.tree, module),
        UseTree::Name(name) => {
            module.names.insert(name.ident.to_string());
        }
        UseTree::Rename(rename) => {
            module.names.insert(rename.rename.to_string());
        }
        UseTree::Glob(_) => module.glob = true,
        UseTree::Group(group) => group.items.iter().for_each(|tree| leaves(tree, module)),
    }
}

/// The public names of a module whose items are `items` and whose child files
/// sit in `dir`.
fn public(items: &[Item], dir: &Path) -> Module {
    let mut module = Module::default();
    for item in items {
        let (vis, name) = match item {
            Item::Use(item) => {
                if is_pub(&item.vis) {
                    leaves(&item.tree, &mut module);
                }
                continue;
            }
            Item::Mod(child) => {
                if is_pub(&child.vis) {
                    let name = child.ident.to_string();
                    let where_ = match &child.content {
                        Some((_, inner)) => Child::Inline(inner.clone()),
                        None => Child::File([
                            dir.join(format!("{name}.rs")),
                            dir.join(&name).join("mod.rs"),
                        ]),
                    };
                    module.modules.insert(name, where_);
                }
                continue;
            }
            Item::Macro(mac) => {
                let exported = mac
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("macro_export"));
                if let (true, Some(ident)) = (exported, &mac.ident) {
                    module.names.insert(ident.to_string());
                }
                continue;
            }
            Item::Const(item) => (&item.vis, item.ident.to_string()),
            Item::Enum(item) => (&item.vis, item.ident.to_string()),
            Item::Fn(item) => (&item.vis, item.sig.ident.to_string()),
            Item::Static(item) => (&item.vis, item.ident.to_string()),
            Item::Struct(item) => (&item.vis, item.ident.to_string()),
            Item::Trait(item) => (&item.vis, item.ident.to_string()),
            Item::Type(item) => (&item.vis, item.ident.to_string()),
            Item::Union(item) => (&item.vis, item.ident.to_string()),
            _ => continue,
        };
        if is_pub(vis) {
            module.names.insert(name);
        }
    }
    module
}

/// Why `krate::segments` does not resolve, or `None` when it does.
fn unresolved(root: &Path, krate: &str, segments: &[String], files: &mut Parsed) -> Option<String> {
    let dir = root.join("crates").join(krate.replace('_', "-"));
    let Some(mut items) = files.items(&dir.join("src/lib.rs")) else {
        return Some(format!("no crate `{krate}` in `crates/`"));
    };
    let mut child_dir = dir.join("src");
    for (depth, segment) in segments.iter().enumerate() {
        let mut module = public(&items, &child_dir);
        if let Some(child) = module.modules.remove(segment) {
            items = match child {
                Child::Inline(inner) => inner,
                Child::File(candidates) => candidates
                    .iter()
                    .find_map(|file| files.items(file))
                    .unwrap_or_default(),
            };
            child_dir = child_dir.join(segment);
            continue;
        }
        if module.names.contains(segment) || module.glob {
            return None;
        }
        let at = if depth == 0 {
            krate.to_owned()
        } else {
            format!("{krate}::{}", segments[..depth].join("::"))
        };
        return Some(format!("`{at}` has no public `{segment}`"));
    }
    None
}

/// Every framework path a rule, a page or a README names resolves to a public
/// item of the crate it names.
#[test]
fn every_path_the_prose_names_resolves() {
    let root: &Path = &repo_root();
    let aliases = umbrella(root);
    let mut files = Parsed::default();
    let mut seen = 0usize;
    let mut holes = BTreeSet::new();
    for file in prose(root) {
        let Ok(text) = read(&file) else {
            continue;
        };
        for (line, krate, segments) in paths_in(&text, &aliases) {
            seen += 1;
            if let Some(why) = unresolved(root, &krate, &segments, &mut files) {
                holes.insert(format!(
                    "{}:{line}: {krate}::{} — {why}",
                    relative(&file, root),
                    segments.join("::"),
                ));
            }
        }
    }
    assert!(
        seen >= FLOOR,
        "read {seen} path(s) — the walk is reading the wrong tree"
    );
    assert!(
        holes.is_empty(),
        "a path the prose names does not resolve; name the public item, or the file when \
         the sentence points at source:\n{}",
        holes.into_iter().collect::<Vec<_>>().join("\n"),
    );
}

/// The reader sees a path through the umbrella, stops at a type, and leaves a
/// span target and a placeholder alone.
#[test]
fn a_path_is_read_through_the_umbrella_and_a_target_is_not_a_path() {
    let aliases = BTreeMap::from([
        ("queue".to_owned(), "nest_rs_queue".to_owned()),
        (
            "oauth::client".to_owned(),
            "nest_rs_oauth_client".to_owned(),
        ),
    ]);
    let text = "`nest_rs::queue::Envelope`, `nest_rs_queue::envelope::open`,\n\
                `nest_rs::oauth::client::OAuthClient`, `nest_rs::operation`,\n\
                `nest_rs_<x>::TARGET`, `nest_rs::<concern>`, `nest_rs::queue`";
    let read: Vec<(usize, String, String)> = paths_in(text, &aliases)
        .into_iter()
        .map(|(line, krate, segments)| (line, krate, segments.join("::")))
        .collect();
    assert_eq!(
        read,
        vec![
            (1, "nest_rs_queue".to_owned(), "Envelope".to_owned()),
            (1, "nest_rs_queue".to_owned(), "envelope::open".to_owned()),
            (
                2,
                "nest_rs_oauth_client".to_owned(),
                "OAuthClient".to_owned()
            ),
        ],
    );

    let root: &Path = &repo_root();
    let files = &mut Parsed::default();
    let path = |segments: &[&str]| segments.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        unresolved(root, "nest_rs_queue", &path(&["Envelope"]), files),
        None
    );
    assert_eq!(
        unresolved(root, "nest_rs_queue", &path(&["consume", "attempt"]), files),
        None,
    );
    assert!(
        unresolved(root, "nest_rs_queue", &path(&["envelope", "open"]), files).is_some(),
        "a private module is not a path",
    );
    assert!(unresolved(root, "nest_rs_nothing", &path(&["X"]), files).is_some());
}
