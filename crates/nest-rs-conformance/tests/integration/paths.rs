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
//! `nest_rs::<segment>…` — the umbrella is a crate like the others, walked the
//! same way, and a segment naming one of its re-exports of a framework crate
//! (`pub use nest_rs_http as http;`) carries the walk into that crate. Each
//! segment is walked through the crate's `lib.rs` and the `pub mod` files under
//! it until it names a public item — a type, a trait, a function, a constant, a
//! re-export — past which the rest is a member the parser would need the type's
//! `impl`s to judge, and is taken on trust. A module holding a glob re-export
//! answers for any name.
//!
//! **A span target is not a path, and the vocabulary says which is which.** A
//! `nest_rs::…` the framework declares as a span target
//! (`sources::declared_targets` — `nest_rs::operation`, `nest_rs::oauth::client`)
//! is a filter directive, left to the `targets` join. Anything else spelled
//! `nest_rs::…` is a path and must resolve: the join used to take every
//! `nest_rs::<word>` that was no umbrella re-export for a target, so a misspelled
//! concern (`nest_rs::shedule::BACKEND_REMEDY`), a renamed one and an item at the
//! umbrella's root (`nest_rs::App`) were never checked — the drift the join
//! exists to catch. A target named in prose that is no longer declared is the
//! same drift, and fails the same way — unless it is one [`RETIRED_TARGETS`]
//! states, which the rules name as history.
//!
//! **Quoted tool output is not prose.** A fenced block marked
//! `frame="terminal"` is what a command printed — rustc's rendering of a type
//! included — and a page quotes it rather than names a path in it.
//!
//! **A module named as a place is a file.** A rule that says a sentence "is
//! worded once in the codegen crate's `job` module" is pointing at source a
//! reader opens, not a path a caller types — so it names the file
//! (`crates/nest-rs-codegen/src/job.rs`), which never pretends to be importable.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use nest_rs_conformance::imports::CrateImports;
use nest_rs_conformance::sources::{
    declared_targets, files_with_extension, parsed, read, relative, repo_root,
};
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

/// Span targets the framework retired, which the rules and the logs page name
/// as history — so a reader holding a filter that names one learns why it
/// matches nothing.
///
/// **Stated, not derived, and it has to be**: a retired target has no
/// declaration left to read it off. Adding a line is retiring a target, which
/// is a reviewed act; a target merely misspelled in prose is never one.
const RETIRED_TARGETS: [&str; 2] = ["nest_rs::access", "nest_rs::access_graph"];

/// Every span target the framework declares or retired, as spelled — the
/// vocabulary a `nest_rs::…` in prose is a filter directive of rather than a
/// path.
fn targets() -> BTreeSet<&'static str> {
    declared_targets()
        .iter()
        .map(|(target, _, _)| *target)
        .chain(RETIRED_TARGETS)
        .collect()
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// Every `nest_rs…::…` path `text` spells, as `(crate, segments)`, with the
/// line it sits on — a declared span target left out.
fn paths_in(text: &str, targets: &BTreeSet<&str>) -> Vec<(usize, String, Vec<String>)> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut terminal = false;
    for (number, line) in text.lines().enumerate() {
        let fence = line.trim_start();
        if fence.starts_with("```") {
            terminal = !terminal && fence.contains("frame=\"terminal\"");
            continue;
        }
        if terminal {
            continue;
        }
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
            if head == "nest_rs"
                && targets.contains(format!("nest_rs::{}", segments.join("::")).as_str())
            {
                continue;
            }
            out.push((number + 1, head.to_owned(), segments));
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

/// Every file the walk parsed, and every crate whose imports it read, so a
/// crate read for its hundredth path is read once.
#[derive(Default)]
struct Parsed {
    files: HashMap<PathBuf, Option<Vec<Item>>>,
    imports: HashMap<PathBuf, CrateImports>,
}

impl Parsed {
    fn items(&mut self, path: &Path) -> Option<Vec<Item>> {
        self.files
            .entry(path.to_owned())
            .or_insert_with(|| parsed(path).map(|file| file.items))
            .clone()
    }

    /// The framework crate a module of the crate at `lib` re-exports whole
    /// under `name` — `pub use nest_rs_http as http;` — read through
    /// `nest_rs_conformance::imports`, the reader the `umbrella` join shares.
    fn reexported_crate(
        &mut self,
        lib: &Path,
        root: &Path,
        module: &[String],
        name: &str,
    ) -> Option<String> {
        let imports = self
            .imports
            .entry(lib.to_owned())
            .or_insert_with(|| CrateImports::read(lib, root));
        imports
            .imports_of(module)
            .find(|(alias, import)| *alias == name && import.public)
            .and_then(|(_, import)| match import.path.as_slice() {
                [krate] if krate.starts_with("nest_rs_") => Some(krate.clone()),
                _ => None,
            })
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
        // A whole framework crate re-exported here — the umbrella's
        // `pub use nest_rs_http as http;` — is walked into, so a path through
        // the front door is held to the crate behind it.
        if segments.len() > depth + 1
            && let Some(target) =
                files.reexported_crate(&dir.join("src/lib.rs"), root, &segments[..depth], segment)
        {
            return unresolved(root, &target, &segments[depth + 1..], files);
        }
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
    let targets = targets();
    let mut files = Parsed::default();
    let mut seen = 0usize;
    let mut holes = BTreeSet::new();
    for file in prose(root) {
        let Ok(text) = read(&file) else {
            continue;
        };
        for (line, krate, segments) in paths_in(&text, &targets) {
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

/// The reader takes a `nest_rs::…` for a path unless the framework declares
/// it as a span target, and walks the umbrella like any other crate.
#[test]
fn a_path_is_read_through_the_umbrella_and_a_target_is_not_a_path() {
    let targets = BTreeSet::from([
        "nest_rs::operation",
        "nest_rs::oauth::client",
        "nest_rs::access",
    ]);
    let text = "`nest_rs::queue::Envelope`, `nest_rs_queue::envelope::open`,\n\
                `nest_rs::oauth::client::OAuthClient`, `nest_rs::operation`,\n\
                `nest_rs_<x>::TARGET`, `nest_rs::<concern>`, `nest_rs::oauth::client`=off\n\
                `nest_rs::storagge::Storage` and `nest_rs::App`; it was `nest_rs::access`\n\
                ```text frame=\"terminal\"\n\
                error: `nest_rs::nest_rs_authn::AuthnGuard` does not check this edge\n\
                ```\n\
                `nest_rs::acces`";
    let read: Vec<(usize, String, String)> = paths_in(text, &targets)
        .into_iter()
        .map(|(line, krate, segments)| (line, krate, segments.join("::")))
        .collect();
    assert_eq!(
        read,
        vec![
            (1, "nest_rs".to_owned(), "queue::Envelope".to_owned()),
            (1, "nest_rs_queue".to_owned(), "envelope::open".to_owned()),
            (
                2,
                "nest_rs".to_owned(),
                "oauth::client::OAuthClient".to_owned()
            ),
            // A word that is no declared target is a path, and is checked.
            (4, "nest_rs".to_owned(), "storagge::Storage".to_owned()),
            (4, "nest_rs".to_owned(), "App".to_owned()),
            (8, "nest_rs".to_owned(), "acces".to_owned()),
        ],
    );

    let root: &Path = &repo_root();
    let files = &mut Parsed::default();
    let path = |segments: &[&str]| segments.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    // Through the umbrella's crate re-exports, at the root and in a family.
    assert_eq!(
        unresolved(root, "nest_rs", &path(&["queue", "Envelope"]), files),
        None
    );
    assert_eq!(
        unresolved(
            root,
            "nest_rs",
            &path(&["oauth", "client", "OAuthClient"]),
            files
        ),
        None,
    );
    assert_eq!(
        unresolved(root, "nest_rs", &path(&["prelude", "App"]), files),
        None
    );
    // The shapes the join used to take for span targets and never check.
    for (segments, why) in [
        (
            &["shedule", "BACKEND_REMEDY"][..],
            "`nest_rs` has no public `shedule`",
        ),
        (
            &["oauth", "resourse", "OAuthResourceModule"],
            "`nest_rs::oauth` has no public `resourse`",
        ),
        (
            &["prelude", "NoSuchThing"],
            "`nest_rs::prelude` has no public `NoSuchThing`",
        ),
        (&["App"], "`nest_rs` has no public `App`"),
        (
            &["queue", "NoSuchThing"],
            "`nest_rs_queue` has no public `NoSuchThing`",
        ),
    ] {
        assert_eq!(
            unresolved(root, "nest_rs", &path(segments), files).as_deref(),
            Some(why),
            "{segments:?}",
        );
    }
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
