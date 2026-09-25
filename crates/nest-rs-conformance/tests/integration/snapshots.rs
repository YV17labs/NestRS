//! The snapshots join: every compile-fail fixture a `.stderr` pins, against the
//! one thing such a snapshot has to be about — the decorator its `//!` describes.
//!
//! A trybuild snapshot is a green test for as long as the compiler prints the
//! same text, whatever that text says. A fixture that does not *parse* never
//! reaches a decorator at all: rustc stops at the syntax error, the `.stderr`
//! pins that, and the refusal the file exists to prove could disappear with the
//! suite still green. Three fixtures in `nest-rs-queue` shipped exactly so — a
//! `use use serde::…` typo, each one's snapshot recording "expected identifier,
//! found keyword `use`" under a `//!` promising the residency refusal.
//!
//! **Parsed with `syn`, never matched on the message.** A parse failure is a
//! property of the fixture, and `syn::parse_file` answers it exactly: a
//! decorator's malformed arguments sit inside an attribute's token stream, which
//! parses as a file, so no legitimate macro diagnostic can land here, and no
//! rustc parser wording can slip past a list of phrases.
//!
//! Members are derived: every `.stderr` under either workspace, paired with the
//! fixture beside it. A fixture that genuinely tests the parser says so in its
//! `//!` with the words `deliberately does not parse`, and is the one exemption.

use std::collections::BTreeSet;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{files_with_extension, read, relative, repo_root};

const BASELINE: &str = "snapshots-baseline.txt";

/// Compile-fail fixtures across both workspaces. Below this the walk is reading
/// the wrong tree.
const FLOOR: usize = 150;

/// The one sentence that exempts a fixture, read off its inner doc comment.
const DELIBERATE: &str = "deliberately does not parse";

#[test]
fn every_compile_fail_fixture_parses_so_its_snapshot_reaches_the_decorator() {
    let root = repo_root();
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;

    for workspace in ["crates", "demo"] {
        for snapshot in files_with_extension(&root.join(workspace), "stderr") {
            let fixture = snapshot.with_extension("rs");
            let Ok(source) = read(&fixture) else {
                continue;
            };
            scanned += 1;
            let declared = source
                .lines()
                .take_while(|line| line.starts_with("//!") || line.trim().is_empty())
                .any(|line| line.contains(DELIBERATE));
            if declared || syn::parse_file(&source).is_ok() {
                continue;
            }
            holes.insert(relative(&fixture, &root));
        }
    }

    baseline::floor(scanned, FLOOR, "compile-fail fixtures");
    baseline::gate(
        BASELINE,
        &holes,
        scanned,
        "compile-fail fixtures",
        "fixtures that do not parse",
        "a snapshot pinning rustc's parse error instead of the diagnostic its \
         `//!` describes — fix the fixture, regenerate the snapshot with \
         `TRYBUILD=overwrite`, and read the new `.stderr` before committing it",
    );
}
