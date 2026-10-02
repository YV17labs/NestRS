//! Every compile-fail snapshot pins the refusal its fixture exists for, never a
//! failure of the fixture's own.
//!
//! A trybuild snapshot stays green for as long as rustc prints the same text,
//! whatever that text says. Two ways a fixture stops reaching its decorator
//! have shipped here, and both leave the snapshot green:
//!
//! - **It does not parse.** Three `nest-rs-queue` fixtures opened with
//!   `use use serde::…`, so their snapshots pinned the parser while their `//!`
//!   promised the residency refusal. `syn::parse_file` answers this exactly: a
//!   decorator's malformed arguments sit inside an attribute's tokens, which
//!   parse as a file, so no legitimate macro diagnostic lands here.
//! - **A name does not resolve.** `transactional_needs_a_value` pinned `E0404`
//!   beside its refusal after `QueueName` became a struct, so the refusal could
//!   change or vanish while the file stayed red. rustc reports most of these
//!   with a code, and an attribute or macro it cannot find with none.
//!
//! A fixture *about* either says so in its `//!` — `deliberately does not
//! parse`, `deliberately fails to resolve`. Members are every `.stderr` under
//! both workspaces, paired with the fixture beside it. What this cannot see is a
//! fixture failing for some other reason of its own (an unrelated `E0308`).

use nest_rs_conformance::sources::{files_with_extension, read, relative, repo_root};

/// Compile-fail fixtures across both workspaces. Below this the walk is reading
/// the wrong tree.
const FLOOR: usize = 150;

/// The sentence that exempts a fixture from the parse check.
const PARSES_ON_PURPOSE: &str = "deliberately does not parse";

/// The sentence that exempts a fixture from the resolution check.
const RESOLVES_ON_PURPOSE: &str = "deliberately fails to resolve";

/// rustc's name-resolution codes: a path, a type, a trait, a value, an import
/// or an associated item that is not where the fixture says it is. None is a
/// refusal the framework words.
const RESOLUTION: [&str; 28] = [
    "E0404", "E0405", "E0407", "E0411", "E0412", "E0422", "E0423", "E0424", "E0425", "E0426",
    "E0430", "E0431", "E0432", "E0433", "E0434", "E0437", "E0438", "E0531", "E0532", "E0573",
    "E0574", "E0575", "E0576", "E0577", "E0578", "E0599", "E0603", "E0659",
];

/// rustc's resolution failures that carry **no** code, as their line opens — an
/// attribute, a derive or a bang macro it cannot find, and one whose resolution
/// it cannot settle. Matched at the start of an `error:` line, so a refusal
/// quoting the words is not taken for one.
const CODELESS: [&str; 2] = [
    "error: cannot find ",
    "error: cannot determine resolution for ",
];

/// Whether the fixture's leading `//!` block carries `sentence`.
fn declares(source: &str, sentence: &str) -> bool {
    source
        .lines()
        .take_while(|line| line.starts_with("//!") || line.trim().is_empty())
        .any(|line| line.contains(sentence))
}

#[test]
fn every_snapshot_pins_the_refusal_its_fixture_exists_for() {
    let root = repo_root();
    let mut holes: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    for workspace in ["crates", "demo"] {
        for snapshot in files_with_extension(&root.join(workspace), "stderr") {
            let fixture = snapshot.with_extension("rs");
            let (Ok(pinned), Ok(source)) = (read(&snapshot), read(&fixture)) else {
                continue;
            };
            scanned += 1;
            let at = relative(&fixture, &root);
            if !declares(&source, PARSES_ON_PURPOSE) && syn::parse_file(&source).is_err() {
                holes.push(format!("{at} does not parse"));
            }
            if declares(&source, RESOLVES_ON_PURPOSE) {
                continue;
            }
            for code in RESOLUTION {
                if pinned.contains(&format!("error[{code}]")) {
                    holes.push(format!("{at} pins {code}"));
                }
            }
            for opening in CODELESS {
                if let Some(line) = pinned.lines().find(|line| line.starts_with(opening)) {
                    holes.push(format!("{at} pins `{}`", line.trim_end()));
                }
            }
        }
    }

    assert!(
        scanned >= FLOOR,
        "read {scanned} compile-fail fixture(s), expected at least {FLOOR}",
    );
    holes.sort();
    assert!(
        holes.is_empty(),
        "{} compile-fail snapshot(s) pin a failure of the fixture's own rather than \
         the refusal it exists for. Fix the fixture, regenerate with \
         `TRYBUILD=overwrite` and read the new `.stderr`; a fixture *about* parsing \
         or resolution says `{PARSES_ON_PURPOSE}` or `{RESOLVES_ON_PURPOSE}` in its \
         `//!`:\n  {}",
        holes.len(),
        holes.join("\n  "),
    );
}
