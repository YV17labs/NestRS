//! The targets join: every crate the two workspaces hold, against a test target
//! that runs it.
//!
//! `CLAUDE.md` fixes the shape — "A test target is always a directory:
//! `tests/<suite>/main.rs`", and exactly two suite names. What nothing asked is
//! whether a crate has *one*, and seventeen do not. Most of that number is a
//! single structural fact rather than seventeen omissions, which is why the
//! exemption below is derived: a `*-macros` crate **cannot** host a snapshot of
//! its own output, because trybuild's fixture has to consume the surface crate
//! that re-exports the decorator, and that is by definition a different crate.
//!
//! So the exemption is not "macros crates are excused". It is: *this* crate
//! exports decorators, and a fixture somewhere exercises one of them. A macros
//! crate landing with no fixture anywhere fails the day it lands, which is the
//! whole difference between a derived exemption and a list.
//!
//! **The obligation is a test that exercises the crate, not a test target of its
//! own** — the workspace-wide clause, and it is not a formality here: read
//! crate-locally this join reports seven holes, six of which are crates whose
//! surface is asserted by name from other crates' suites. That is precisely the
//! failure the rule describes ("one crate here was reported untested while its
//! whole public surface was asserted from four other crates"), and closing those
//! six would mean six duplicate suites.
//!
//! **Four routes, not one.** A crate is exercised by a test target of its own,
//! by its own `#[cfg(test)]` unit tests, by another crate's suite naming its
//! path, or — for a proc-macro crate — by a fixture exercising a decorator it
//! exports. Leaving out unit tests was this join's second wrong reading, and it
//! cost the same as the first: two crates reported untested while forty-two and
//! ten assertions respectively covered them.
//!
//! The member is spelled by its directory name; its surface is spelled the way
//! a caller writes it, `nest_rs_<x>::…`; a decorator is spelled the way a
//! fixture writes it, as a bare identifier under `#[…]`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, executed_tokens, exported_decorators, idents, path_roots, read, relative,
    repo_root, rust_files, suite_runs_tests,
};
use proc_macro2::TokenStream;

const BASELINE: &str = "targets-baseline.txt";

/// Both workspaces together stand well above this; below it the scan is reading
/// the wrong tree and every hole it reports is an artefact.
const FLOOR: usize = 40;

/// The two suite names `CLAUDE.md` permits. A third is not a hole this join
/// reports — it is the locked layout being broken, and the assertion says so
/// separately.
const SUITES: [&str; 2] = ["integration", "e2e"];

fn crate_name(dir: &Path) -> String {
    dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned()
}

/// Whether the crate carries a test target of the sanctioned shape.
///
/// A flat `tests/<x>.rs` deliberately does not count: Cargo compiles it as its
/// own binary, which escapes the nextest gates — the reason the layout is locked.
fn has_test_target(dir: &Path) -> bool {
    SUITES.iter().any(|suite| {
        let target = dir.join("tests").join(suite);
        // The **shape** — `main.rs`, never a flat `tests/<x>.rs` — and then
        // that something in it runs. Asking only whether `main.rs` exists left
        // the cell green over a file truncated to zero bytes, which is a crate
        // with a test target and no tests: exactly the state this join says it
        // has one to rule out.
        suite_runs_tests(&target.join("main.rs"))
    })
}

/// Every identifier a trybuild fixture spells, workspace-wide.
///
/// A fixture is the input rustc is handed, so it need not — and often does not —
/// parse; it is read as tokens for that reason, and a file too broken even to
/// tokenize simply contributes nothing.
fn fixture_idents(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for dir in crate_dirs() {
        for path in rust_files(&dir.join("tests")) {
            if !relative(&path, root).contains("/diagnostics/") {
                continue;
            }
            let Ok(text) = read(&path) else {
                continue;
            };
            let Ok(tokens) = text.parse::<TokenStream>() else {
                continue;
            };
            out.extend(idents(tokens));
        }
    }
    out
}

/// Every crate path an executed test reaches into, workspace-wide.
///
/// `executed_tokens` is what makes this an assertion rather than a mention: a
/// trybuild fixture is excluded, and inside `src/` only `#[cfg(test)]` counts.
fn reached_crates(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for dir in crate_dirs() {
        for path in rust_files(&dir.join("tests")) {
            for tokens in executed_tokens(&path, root) {
                out.extend(path_roots(&tokens));
            }
        }
    }
    out
}

/// Whether the crate's own `src` carries executed unit tests.
///
/// `CLAUDE.md` sanctions them outright — "Unit tests are untouched:
/// `#[cfg(test)] mod tests` in the file under test" — so a crate covered that
/// way is covered, and the first reading of this join said otherwise. It
/// reported `nest-rs-codegen` (42 unit tests) and `nest-rs-server-timing` (10)
/// as untested, and closing those two cells would have meant writing integration
/// suites over functions already asserted, which is the "no second test for an
/// occupied cell" clause broken by the suite that exists to state it.
fn has_unit_tests(dir: &Path, root: &Path) -> bool {
    rust_files(&dir.join("src"))
        .iter()
        .any(|path| !executed_tokens(path, root).is_empty())
}

/// A crate no test exercises, by any of the four routes.
fn is_hole(
    dir: &Path,
    root: &Path,
    fixtures: &BTreeSet<String>,
    reached: &BTreeSet<String>,
) -> bool {
    if has_test_target(dir) {
        return false;
    }
    if has_unit_tests(dir, root) {
        return false;
    }
    if reached.contains(&crate_name(dir).replace('-', "_")) {
        return false;
    }
    let decorators = exported_decorators(dir);
    if decorators.is_empty() {
        // Not a proc-macro crate, so nothing stops it hosting its own suite.
        return true;
    }
    !decorators.iter().any(|d| fixtures.contains(d))
}

#[test]
fn every_crate_has_a_test_target_or_a_derived_reason_not_to() {
    let root = repo_root();
    let crates: Vec<PathBuf> = crate_dirs();
    baseline::floor(crates.len(), FLOOR, "crate(s) across the two workspaces");

    // The locked layout, checked here because this join already walks every
    // `tests/` in the repo: a third suite name or a flat `tests/<x>.rs` is not a
    // hole to record, it is the norm being broken.
    let mut stray: Vec<String> = Vec::new();
    for dir in &crates {
        for entry in std::fs::read_dir(dir.join("tests"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let is_flat_target = path.extension().is_some_and(|x| x == "rs");
            let is_third_suite = path.is_dir() && !SUITES.contains(&name);
            if is_flat_target || is_third_suite {
                stray.push(relative(&path, &root));
            }
        }
    }
    stray.sort();
    assert!(
        stray.is_empty(),
        "{} test target(s) outside the two sanctioned suites — `CLAUDE.md` fixes \
         the layout as `tests/integration/main.rs` and `tests/e2e/main.rs`, and \
         a flat `tests/<x>.rs` compiles as its own binary, escaping the nextest \
         gates:\n  {}",
        stray.len(),
        stray.join("\n  "),
    );

    let fixtures = fixture_idents(&root);
    let reached = reached_crates(&root);
    let holes: BTreeSet<String> = crates
        .iter()
        .filter(|dir| is_hole(dir, &root, &fixtures, &reached))
        .map(|dir| crate_name(dir))
        .collect();

    baseline::gate(
        BASELINE,
        &holes,
        crates.len(),
        "crates",
        "crate(s)",
        "a crate no test target covers",
    );
}
