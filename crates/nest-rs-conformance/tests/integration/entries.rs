//! The entries join: every entry point the repository writes runs on
//! `#[nest_rs::main]`.
//!
//! The decorator is what bounds the last step of the way down — the runtime's
//! teardown, held to what the shutdown hooks left of their budget (`CLAUDE.md`,
//! *No wait the framework does not bound*). `#[tokio::main]` builds the same
//! runtime and drops it as `main` returns, and that drop waits for every
//! blocking task still running: a hook the budget abandoned while it waited on a
//! `spawn_blocking` held the exit past every bound, after the last line. Both
//! compile, so nothing but this join tells them apart.
//!
//! **Members are derived, never listed**: every function in a Rust source under
//! `crates/`, `demo/` and `bench/` — suites and fixtures included, since a test
//! binary tears a runtime down like any other — and every example in a doc
//! comment there, which is what a developer copies. Two readings:
//!
//! - **an `async fn main` carries the framework's decorator, written in full** —
//!   `nest_rs::main`, or `nest_rs_core::main` in a framework crate below the
//!   umbrella. A bare `#[main]` is refused with the rest: `use tokio::main;`
//!   makes it tokio's, and this join reads no imports;
//! - **`#[tokio::main]` is refused on any function**, whatever its name.
//!
//! A synchronous `main` is outside the population — it builds no runtime unless
//! its body says so, and `nest-rs-cli`'s is one. Markdown is read for the one
//! line that can only be code, an attribute line opening `#[tokio::main]`, in
//! the documentation site and every README. The CLI's templates are strings
//! rather than items, so the CLI's own suite scans what they emit
//! (`templates::tests`), the precedent `env_names` states.
//!
//! **What it reads by its spelling**: the `main` attribute, and the `main`
//! function. The `blinds` join refuses a rename of either.

use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    doctests, files_with_extension, parsed, read, relative, repo_root, rust_files,
};
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![Followed::attribute(MAIN), Followed::declaration(MAIN)]
}

/// The entry point's name, and the attribute's last segment.
const MAIN: &str = "main";

/// The decorator's two full spellings: through the umbrella, as every app
/// writes it, and through the kernel, for a framework crate below the umbrella.
const ACCEPTED: [[&str; 2]; 2] = [["nest_rs", MAIN], ["nest_rs_core", MAIN]];

/// The runtime macro whose teardown is unbounded.
const REFUSED: [&str; 2] = ["tokio", MAIN];

/// The trees whose Rust the repository writes: the two workspaces and the
/// benchmark's system under test.
const TREES: [&str; 3] = ["crates", "demo", "bench"];

/// Below this the walk is reading the wrong tree: five demo apps, the two demo
/// tools and the benchmark are eight entry points.
const FLOOR: usize = 8;

/// An attribute's path, as its segments.
fn segments(attr: &syn::Attribute) -> Vec<String> {
    attr.path()
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

/// What one function's attributes say about its runtime.
#[derive(Default)]
struct Scan {
    file: String,
    /// `async fn main`s read, refused or not.
    entries: usize,
    refused: Vec<String>,
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let attributes: Vec<Vec<String>> = node.attrs.iter().map(segments).collect();
        let name = &node.sig.ident;
        if attributes.iter().any(|path| *path == REFUSED) {
            self.refused.push(format!(
                "`fn {name}` in {} is `#[tokio::main]`, whose runtime drop waits on every \
                 blocking task still running — write `#[nest_rs::main]`, which tears the \
                 runtime down within the shutdown budget",
                self.file,
            ));
        } else if name == MAIN && node.sig.asyncness.is_some() {
            self.entries += 1;
            let decorated = attributes
                .iter()
                .any(|path| ACCEPTED.iter().any(|accepted| path == accepted));
            if !decorated {
                self.refused.push(format!(
                    "the `async fn main` in {} is not `#[nest_rs::main]`, written in full — \
                     whatever runtime it runs on is torn down without the shutdown budget's \
                     bound",
                    self.file,
                ));
            }
        }
        syn::visit::visit_item_fn(self, node);
    }
}

/// Every refusal over the Rust under `root`'s trees, and the entry points read.
fn scan(root: &Path) -> Scan {
    let mut scan = Scan::default();
    for tree in TREES {
        for path in rust_files(&root.join(tree)) {
            let Some(ast) = parsed(&path) else {
                continue;
            };
            let rel = relative(&path, root);
            scan.file = rel.clone();
            scan.visit_file(&ast);
            scan.file = format!("{rel} (doctest)");
            for example in doctests(&ast) {
                scan.visit_file(&example);
            }
        }
    }
    scan
}

/// Every Markdown line under `root` that opens `#[tokio::main]` — an attribute
/// line, which prose quoting the attribute in backticks never is.
fn markdown_entries(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for tree in TREES.iter().chain(&["docs"]) {
        for extension in ["md", "mdx"] {
            for path in files_with_extension(&root.join(tree), extension) {
                let Ok(text) = read(&path) else {
                    continue;
                };
                for (at, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("#[tokio::main]") {
                        found.push(format!(
                            "{}:{} shows `#[tokio::main]` — an example teaches the unbounded \
                             teardown; show `#[nest_rs::main]`",
                            relative(&path, root),
                            at + 1,
                        ));
                    }
                }
            }
        }
    }
    found
}

#[test]
fn every_entry_point_runs_on_the_framework_s_runtime() {
    let root = repo_root();
    let scan = scan(&root);
    baseline::floor(scan.entries, FLOOR, "`async fn main` entry point(s)");
    let mut refused = scan.refused;
    refused.extend(markdown_entries(&root));
    assert!(
        refused.is_empty(),
        "{} entry point(s) whose runtime's teardown nothing bounds:\n  {}",
        refused.len(),
        refused.join("\n  "),
    );
}

/// The reading itself, on a tree whose answer is written beside it: the two
/// full spellings pass, tokio's is refused on any function, and a bare
/// `#[main]` or an undecorated `async fn main` is refused; a synchronous `main`
/// is not read at all.
#[test]
fn an_entry_point_is_read_off_its_attribute_and_tokio_s_is_refused() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("entries-planted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(
        &root,
        &[
            (
                "crates/a/src/main.rs",
                "#[nest_rs::main] async fn main() {}",
            ),
            (
                "crates/b/src/main.rs",
                "#[nest_rs_core::main] async fn main() {}",
            ),
            ("crates/c/src/main.rs", "fn main() {}"),
            (
                "demo/apps/d/src/main.rs",
                "#[tokio::main] async fn main() {}",
            ),
            (
                "demo/apps/e/src/main.rs",
                "use tokio::main; #[main] async fn main() {}",
            ),
            ("demo/apps/f/src/lib.rs", "#[tokio::main] async fn run() {}"),
            ("bench/g/src/main.rs", "async fn main() {}"),
            (
                "docs/page.mdx",
                "Not `#[tokio::main]`:\n\n```rust\n#[tokio::main]\n```\n",
            ),
        ],
    );

    let scan = scan(&root);
    assert_eq!(scan.entries, 4, "the async mains read: {:#?}", scan.refused);
    let refused = |file: &str| scan.refused.iter().any(|line| line.contains(file));
    assert!(!refused("crates/a/"), "{:#?}", scan.refused);
    assert!(!refused("crates/b/"), "{:#?}", scan.refused);
    assert!(!refused("crates/c/"), "{:#?}", scan.refused);
    assert!(refused("demo/apps/d/"), "{:#?}", scan.refused);
    assert!(refused("demo/apps/e/"), "{:#?}", scan.refused);
    assert!(refused("demo/apps/f/"), "{:#?}", scan.refused);
    assert!(refused("bench/g/"), "{:#?}", scan.refused);
    assert_eq!(
        markdown_entries(&root),
        vec![
            "docs/page.mdx:4 shows `#[tokio::main]` — an example teaches the unbounded \
             teardown; show `#[nest_rs::main]`"
                .to_owned()
        ],
    );
}
