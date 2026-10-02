//! The test-target layout: every crate's `tests/` holds the two sanctioned
//! suites and nothing else.
//!
//! `CLAUDE.md` locks it — a test target is always `tests/<suite>/main.rs`, and
//! the suite is `integration` or `e2e`. A flat `tests/<x>.rs` compiles as a
//! binary of its own, escaping the nextest gates, and a third suite name is a
//! binary the `binary(e2e)` filter neither selects nor excludes on purpose.
//!
//! Read off directory entries, so nothing in a crate's code can hide one.
//! Whether a crate is *exercised* is not asked here: line coverage
//! (`cargo llvm-cov nextest`) answers that semantically.

use nest_rs_conformance::sources::{crate_dirs, relative, repo_root};

/// The two suite names `CLAUDE.md` permits.
const SUITES: [&str; 2] = ["integration", "e2e"];

/// Both workspaces together stand well above this; below it the walk is reading
/// the wrong tree.
const FLOOR: usize = 40;

#[test]
fn every_test_target_is_a_sanctioned_suite() {
    let root = repo_root();
    let crates = crate_dirs();
    assert!(
        crates.len() >= FLOOR,
        "read {} crate(s) across the two workspaces, expected at least {FLOOR}",
        crates.len(),
    );

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
}
