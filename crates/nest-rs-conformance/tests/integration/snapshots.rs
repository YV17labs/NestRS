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
//!
//! **Parsing is the first of two ways a fixture stops reaching its decorator.**
//! The second is name resolution: a fixture that parses but whose `use` no
//! longer resolves, or which implements a trait that became a struct, still
//! expands the decorator — and its snapshot then pins the resolution error
//! *beside* the refusal, so the refusal can change or vanish while rustc's
//! `E0404` keeps the file red in the same way. `transactional_needs_a_value`
//! shipped so after `QueueName` became a struct. The second test refuses any
//! snapshot carrying a name-resolution error, unless the fixture's `//!` says
//! `deliberately fails to resolve`.
//!
//! **A resolution error is read by code where rustc gives one, and by sentence
//! where it does not.** rustc reports an attribute, a derive or a bang macro it
//! cannot find with no code at all — `error: cannot find attribute `processr`
//! in this scope` — and that is the likeliest rot in a decorator suite: a
//! renamed decorator under a glob import. Matching codes alone passed it.
//!
//! **It reads no name by its spelling**, so it declares none to the `blinds`
//! join: its members are `.stderr` files and the fixture beside each.

use std::collections::BTreeSet;
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{files_with_extension, read, relative, repo_root};

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    Vec::new()
}

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

/// The one sentence that exempts a fixture from the resolution check.
const RESOLVES_ON_PURPOSE: &str = "deliberately fails to resolve";

/// rustc's name-resolution codes: a path, a type, a trait, a value, an import
/// or an associated item that is not where the fixture says it is. None is a
/// refusal the framework words, so a snapshot carrying one pins a fixture that
/// stopped compiling for a reason of its own.
const RESOLUTION: [&str; 28] = [
    "E0404", // expected trait, found something else
    "E0405", // cannot find trait
    "E0407", // method is not a member of the trait
    "E0411", // `Self` used outside an impl or a trait
    "E0412", // cannot find type
    "E0422", // cannot find struct, variant or union
    "E0423", // expected value, found something else
    "E0424", // `self` is not available here
    "E0425", // cannot find value
    "E0426", // undeclared label
    "E0430", // `self` import appears more than once
    "E0431", // `self` import in an empty prefix
    "E0432", // unresolved import
    "E0433", // failed to resolve
    "E0434", // cannot capture a dynamic environment in a fn item
    "E0437", // type is not a member of the trait
    "E0438", // const is not a member of the trait
    "E0531", // cannot find tuple struct or variant
    "E0532", // expected tuple struct or variant
    "E0573", // expected type, found something else
    "E0574", // expected a struct, variant or union type, found something else
    "E0575", // expected an associated type, found something else
    "E0576", // associated item not found in the trait
    "E0577", // expected a module, found something else
    "E0578", // cannot determine the type of a path
    "E0599", // no method or associated item found
    "E0603", // item is private
    "E0659", // a name is ambiguous
];

/// rustc's resolution failures that carry **no** code, as their sentences open:
/// an attribute, a derive and a bang macro it cannot find, and a macro whose
/// resolution it cannot settle. Matched at the start of an `error:` line, so a
/// refusal that quotes the words in its own sentence is not taken for one.
const CODELESS: [&str; 2] = [
    "error: cannot find ",
    "error: cannot determine resolution for ",
];

/// Every fixture under `root`'s two workspaces whose snapshot pins a
/// name-resolution error it does not declare, and how many fixtures were read.
fn unresolved_fixtures(root: &Path) -> (BTreeSet<String>, usize) {
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;
    for workspace in ["crates", "demo"] {
        for snapshot in files_with_extension(&root.join(workspace), "stderr") {
            let (Ok(pinned), Ok(source)) = (read(&snapshot), read(&snapshot.with_extension("rs")))
            else {
                continue;
            };
            scanned += 1;
            let declared = source
                .lines()
                .take_while(|line| line.starts_with("//!") || line.trim().is_empty())
                .any(|line| line.contains(RESOLVES_ON_PURPOSE));
            if declared {
                continue;
            }
            for code in RESOLUTION {
                if pinned.contains(&format!("error[{code}]")) {
                    holes.insert(format!(
                        "{} pins {code}",
                        relative(&snapshot.with_extension("rs"), root)
                    ));
                }
            }
            for opening in CODELESS {
                if let Some(line) = pinned.lines().find(|line| line.starts_with(opening)) {
                    holes.insert(format!(
                        "{} pins `{}`",
                        relative(&snapshot.with_extension("rs"), root),
                        line.trim_end(),
                    ));
                }
            }
        }
    }
    (holes, scanned)
}

#[test]
fn no_snapshot_pins_a_name_resolution_error_its_fixture_does_not_declare() {
    let root = repo_root();
    let (holes, scanned) = unresolved_fixtures(&root);
    baseline::floor(scanned, FLOOR, "compile-fail fixtures");
    // No baseline: the one live case was the reason this half was written, and
    // it was fixed in the same change.
    assert!(
        holes.is_empty(),
        "{} compile-fail fixture(s) pin a name-resolution error beside the \
         refusal they exist for — the fixture stopped compiling for a reason of \
         its own, so the refusal it promises is no longer what keeps it red. Fix \
         the fixture, regenerate with `TRYBUILD=overwrite` and read the new \
         `.stderr`; a fixture *about* resolution says `{RESOLVES_ON_PURPOSE}` in \
         its `//!`:\n  {}",
        holes.len(),
        holes.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );
}

/// The join's verdict on a planted tree: the shape that shipped, a fixture
/// declaring it on purpose, and a clean one.
#[test]
fn a_fixture_pinning_a_resolution_error_is_found_unless_it_says_so() {
    const TREE: [(&str, &str); 10] = [
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/rotted.rs",
            "//! Promises the residency refusal.\nuse nest_rs_queue::processr;\nfn main() {}\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/rotted.stderr",
            "error: the refusal\n\nerror[E0432]: unresolved import `nest_rs_queue::processr`\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/on_purpose.rs",
            "//! A path the decorator emits deliberately fails to resolve here.\nfn main() {}\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/on_purpose.stderr",
            "error[E0433]: failed to resolve\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/clean.rs",
            "//! The refusal alone.\nfn main() {}\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/clean.stderr",
            "error: the refusal\n\nerror[E0277]: `X` does not check GraphQL operations\n",
        ),
        // The codeless sentences: an attribute renamed under a glob import, and
        // a bang macro.
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/attr_rot.rs",
            "//! Promises the residency refusal.\nuse nest_rs_queue::*;\n#[processr] fn main() {}\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/attr_rot.stderr",
            "error: the refusal\n\nerror: cannot find attribute `processr` in this scope\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/macro_rot.rs",
            "//! Promises the enqueue refusal.\nfn main() { enqueu!(); }\n",
        ),
        (
            "crates/nest-rs-probe/tests/integration/diagnostics/macro_rot.stderr",
            "error: cannot find macro `enqueu` in this scope\n",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("snapshots-resolution-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let (holes, scanned) = unresolved_fixtures(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(scanned, 5);
    assert_eq!(
        holes.into_iter().collect::<Vec<_>>(),
        [
            "crates/nest-rs-probe/tests/integration/diagnostics/attr_rot.rs pins `error: cannot \
             find attribute `processr` in this scope`",
            "crates/nest-rs-probe/tests/integration/diagnostics/macro_rot.rs pins `error: cannot \
             find macro `enqueu` in this scope`",
            "crates/nest-rs-probe/tests/integration/diagnostics/rotted.rs pins E0432",
        ],
    );
}
