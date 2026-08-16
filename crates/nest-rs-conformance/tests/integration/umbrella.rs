//! The umbrella join: every capability the front door offers, against the
//! witnesses `CLAUDE.md` makes it owe.
//!
//! *The umbrella is the front door* fixes what shipping a capability costs — a
//! feature in the matrix, a `pub use`, `cargo add nest-rs --features <x>` in the
//! README **and** the docs page's `## Install`, and two witnesses. It reads as a
//! checklist and was checked by nobody, so a capability could reach a user with
//! no documented way to install it.
//!
//! A **capability** is derived, not listed: a feature whose value activates a
//! `dep:nest-rs-<y>`. That is what separates the twenty-eight capabilities from
//! `default` and `full`, which activate nothing of their own.
//!
//! Four columns, and each is owed by a derived subset rather than by all
//! twenty-eight:
//!
//! - **the re-export**, owed by every capability — `pub use nest_rs_<y> as <y>`,
//!   which is what makes `::nest_rs::<y>::` resolve inside a macro expansion;
//! - **the docs install line**, owed by every capability — some page's
//!   `## Install` spelling `cargo add nest-rs --features <x>`;
//! - **the README**, owed by every capability, and it is *two* checks because
//!   the rule is two sentences: the capability's own README has to spell
//!   `cargo add nest-rs --features <x>`, and no README may tell a reader to
//!   `cargo add nest-rs-<x>` instead. Only the second was implemented once, and
//!   a negative check alone passes vacuously — an empty README corpus reported
//!   zero holes, which is the shape a scan reading the wrong directory takes;
//! - **the expansion witness**, owed only by a capability that *ships
//!   decorators* — derived by asking whether `crates/nest-rs-<y>-macros` exists,
//!   which is exactly the ten of twenty-eight `nest-rs-macro-hygiene` enables.
//!
//! The fifth obligation — a boot executed in the owning crate — is the seams
//! join's whole subject, and a cell it already reports is closed. Writing it
//! twice is the "no second test for an occupied cell" clause being broken by the
//! very suite that exists to state it.
//!
//! **One limit of the expansion column, reported rather than papered over**:
//! `nest-rs-macro-hygiene` turns its ten features on in a single build, so a
//! satellite that one capability fails to pull is invisible whenever a
//! neighbouring capability pulls it. This join proves a use site exists; it
//! cannot prove that use site would still compile alone. Splitting the witness
//! per feature is an owner decision, not something to infer from here.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, exported_decorators, files_with_extension, idents, parsed, read, repo_root,
    rust_files,
};
use syn::{Item, UseTree};

const BASELINE: &str = "umbrella-baseline.txt";

/// Twenty-eight capabilities stand today. Below that the scan is reading the
/// wrong manifest and every hole it reports is an artefact.
const FLOOR: usize = 28;

/// A capability, as the umbrella's own feature matrix declares it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Capability {
    /// The feature name — `server-timing`, the way `--features` spells it.
    feature: String,
    /// The crate it activates — `nest-rs-server-timing`.
    krate: String,
}

impl Capability {
    /// The module path both halves of the re-export use — `server_timing`.
    fn concern(&self) -> String {
        self.krate
            .strip_prefix("nest-rs-")
            .unwrap_or(&self.krate)
            .replace('-', "_")
    }

    fn cell(&self, column: &str) -> String {
        format!("{} :: {column}", self.feature)
    }
}

/// Every capability the umbrella declares, from its `[features]` table.
///
/// Parsed with a TOML parser rather than scanned: a feature list wraps across
/// lines as freely as a Rust string does, and the wrapping is exactly what a
/// line-oriented read gets wrong.
fn capabilities(root: &Path) -> Vec<Capability> {
    let manifest =
        read(&root.join("crates/nest-rs/Cargo.toml")).expect("the umbrella has a manifest");
    let doc: toml_edit::DocumentMut = manifest
        .parse()
        .expect("the umbrella manifest is valid TOML");
    let Some(features) = doc.get("features").and_then(|f| f.as_table()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (feature, value) in features {
        let Some(list) = value.as_array() else {
            continue;
        };
        for entry in list {
            let Some(text) = entry.as_str() else {
                continue;
            };
            // `dep:nest-rs-x` activates the crate; `nest-rs-x?/y` only forwards a
            // feature to a crate some *other* capability activates, so it never
            // makes this feature the owner.
            if let Some(krate) = text.strip_prefix("dep:") {
                out.push(Capability {
                    feature: feature.to_owned(),
                    krate: krate.to_owned(),
                });
            }
        }
    }
    out
}

/// The concerns the umbrella re-exports at its root, as `<crate> as <alias>`.
fn reexports(root: &Path) -> BTreeSet<(String, String)> {
    let Some(ast) = parsed(&root.join("crates/nest-rs/src/lib.rs")) else {
        return BTreeSet::new();
    };
    ast.items
        .iter()
        .filter_map(|item| {
            let Item::Use(use_item) = item else {
                return None;
            };
            // `pub use nest_rs_http as http;` — a bare rename, no `::` in it,
            // which is a `UseTree::Rename` at the top of the tree rather than
            // the `Path` a qualified import would give.
            let UseTree::Rename(rename) = &use_item.tree else {
                return None;
            };
            Some((rename.ident.to_string(), rename.rename.to_string()))
        })
        .collect()
}

/// Every `cargo add nest-rs --features …` line the docs spell under `## Install`.
///
/// Scoped to that section rather than to the page, because the rule names the
/// section: a feature mentioned in prose lower down is not an install line.
fn documented_installs(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for page in files_with_extension(&root.join("docs/src/content/docs"), "mdx") {
        let Ok(text) = read(&page) else {
            continue;
        };
        let mut inside = false;
        for line in text.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                inside = heading.trim() == "Install";
                continue;
            }
            if !inside {
                continue;
            }
            // Two things a stricter match got wrong. The comma:
            // `--features graphql,seaorm,authz` is one line declaring three
            // capabilities, and that is how the docs write it. And the flags:
            // `testing/index.mdx` says `cargo add --dev nest-rs --features
            // testing`, which is the *correct* advice for a test-only
            // dependency — anchoring on the literal `cargo add nest-rs` read it
            // as a page with no install line at all.
            if !line.contains("cargo add") {
                continue;
            }
            if let Some((_, rest)) = line.split_once("nest-rs --features") {
                out.extend(
                    rest.split([' ', ',', '\t'])
                        .filter(|token| !token.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
    out
}

/// Every README in the repo, keyed by the crate that owns it — the crates.io
/// landing pages plus the root's, since the negative half of the rule binds the
/// sentence wherever it appears while the positive half binds one page.
///
/// The root is keyed by the empty string: it belongs to no capability, so it can
/// never satisfy the positive half by accident.
fn readmes(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = read(&root.join("README.md")) {
        out.push((String::new(), text));
    }
    for dir in crate_dirs() {
        let Some(krate) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Ok(text) = read(&dir.join("README.md")) {
            out.push((krate.to_owned(), text));
        }
    }
    assert!(
        out.len() > FLOOR,
        "{} README(s) read — the corpus is the population's own size at least, \
         and a negative column over an empty corpus reports nothing",
        out.len(),
    );
    out
}

/// Every identifier `nest-rs-macro-hygiene`'s use sites spell.
fn hygiene_idents(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for path in rust_files(&root.join("crates/nest-rs-macro-hygiene/src")) {
        let Ok(text) = read(&path) else {
            continue;
        };
        let Ok(tokens) = text.parse::<proc_macro2::TokenStream>() else {
            continue;
        };
        out.extend(idents(tokens));
    }
    out
}

#[test]
fn every_umbrella_capability_carries_its_witnesses() {
    let root = repo_root();
    let capabilities = capabilities(&root);
    baseline::floor(
        capabilities.len(),
        FLOOR,
        "capability feature(s) in the umbrella manifest",
    );

    let reexports = reexports(&root);
    let installs = documented_installs(&root);
    let readmes = readmes(&root);
    let hygiene = hygiene_idents(&root);

    let mut holes = BTreeSet::new();
    for cap in &capabilities {
        let concern = cap.concern();
        if !reexports.contains(&(cap.krate.replace('-', "_"), concern.clone())) {
            holes.insert(cap.cell("pub use"));
        }
        if !installs.contains(&cap.feature) {
            holes.insert(cap.cell("docs ## Install"));
        }
        let subcrate = format!("cargo add {}", cap.krate);
        if readmes.iter().any(|(_, text)| text.contains(&subcrate)) {
            holes.insert(cap.cell("README installs the sub-crate"));
        }
        // The positive half, on the capability's **own** landing page — the one
        // a crates.io visitor reads. `--dev` is accepted: `nest-rs-testing`
        // installs that way and the rule's point is the umbrella, not the table.
        let own = readmes
            .iter()
            .find(|(krate, _)| *krate == cap.krate)
            .map(|(_, text)| text.as_str())
            .unwrap_or_default();
        let installs_umbrella = own
            .contains(&format!("cargo add nest-rs --features {}", cap.feature))
            || own.contains(&format!(
                "cargo add --dev nest-rs --features {}",
                cap.feature
            ));
        if !installs_umbrella {
            holes.insert(cap.cell("README installs the umbrella"));
        }
        // Only a capability that ships decorators owes an expansion witness; the
        // existence of its `*-macros` crate is what says it does.
        let macros = root.join("crates").join(format!("{}-macros", cap.krate));
        if macros.is_dir() {
            let decorators = exported_decorators(&macros);
            if !decorators.is_empty() && !decorators.iter().any(|d| hygiene.contains(d)) {
                holes.insert(cap.cell("nest-rs-macro-hygiene use site"));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        capabilities.len(),
        "capabilities",
        "umbrella capability × witness",
        "a witness the front door owes",
    );
}
