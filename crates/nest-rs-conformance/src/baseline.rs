//! The cells a join found empty on the day it landed.
//!
//! A join cannot be introduced into a codebase that predates it without one:
//! the alternative is a red suite until every pre-existing cell is filled,
//! which is how a check gets deleted instead of obeyed. So today's holes are
//! recorded once and the join fails on the *next* one.
//!
//! **It only shrinks** — the docs linter's contract, for the same reason. A
//! line removed is a cell filled; a line added is a regression argued in
//! review rather than absorbed by a flag. There is deliberately no
//! `--update` switch: adding a line is an edit a reader sees.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// What a join concluded about its family, ready to be asserted on.
pub struct Verdict {
    /// Holes the baseline does not excuse — the regression.
    pub fresh: Vec<String>,
    /// Baseline lines that name a cell now filled — delete them.
    pub stale: Vec<String>,
}

impl Verdict {
    pub fn is_clean(&self) -> bool {
        self.fresh.is_empty() && self.stale.is_empty()
    }

    /// The whole failure in one message: what regressed, then what to delete.
    /// Both halves at once, because fixing one and rerunning to discover the
    /// other is how a two-line chore becomes two rounds.
    pub fn report(&self, family: &str) -> String {
        let mut out = String::new();
        if !self.fresh.is_empty() {
            out.push_str(&format!(
                "{} {family} covered by no test:\n  {}\n\nCover them where the \
                 behaviour is tested. The baseline records what was already \
                 uncovered when this join landed; it never grows.\n",
                self.fresh.len(),
                self.fresh.join("\n  "),
            ));
        }
        if !self.stale.is_empty() {
            out.push_str(&format!(
                "\n{} baseline line(s) name a {family} cell that is now covered \
                 — delete them, the baseline only shrinks:\n  {}\n",
                self.stale.len(),
                self.stale.join("\n  "),
            ));
        }
        out
    }
}

/// Compare today's holes against the recorded ones.
///
/// Returns `None` when no baseline exists yet: the caller writes one and fails,
/// so the landing is a reviewed commit rather than a silent green.
pub fn compare(path: &Path, holes: &BTreeSet<String>) -> Option<Verdict> {
    let raw = fs::read_to_string(path).ok()?;
    let recorded: BTreeSet<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    Some(Verdict {
        fresh: holes.difference(&recorded).cloned().collect(),
        stale: recorded.difference(holes).cloned().collect(),
    })
}

/// Write the initial baseline. Only ever called on the run that finds none.
pub fn land(path: &Path, holes: &BTreeSet<String>) {
    let listing = holes.iter().cloned().collect::<Vec<_>>().join("\n");
    fs::write(path, format!("{listing}\n")).expect("the baseline path is writable");
}

/// The floor a join names for its own population, asserted with one sentence.
///
/// Every join owes this check — a scan reading the wrong tree reports its whole
/// family as holes, and a green baseline diff is the worst way to find out. The
/// sentence is here rather than per join so the seven copies cannot drift into
/// seven explanations of the same failure.
#[track_caller]
pub fn floor(found: usize, floor: usize, unit: &str) {
    assert!(
        found >= floor,
        "the scan found {found} {unit} — below {floor} it is reading the wrong \
         tree, and every hole it reports is an artefact",
    );
}

/// The whole tail of a join: land a first baseline and fail, or compare and
/// assert.
///
/// `file` is the baseline's name inside `tests/integration/`, resolved against
/// this crate's own manifest — the baselines and this module ship in the same
/// crate, so no join has to spell the path. `scanned` and `unit` are the
/// population the join walked, and `line_is` completes "every line is …", which
/// is the one sentence a first landing exists to make a reader act on.
#[track_caller]
pub fn gate(
    file: &str,
    holes: &BTreeSet<String>,
    scanned: usize,
    unit: &str,
    family: &str,
    line_is: &str,
) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/integration")
        .join(file);
    let Some(verdict) = compare(&path, holes) else {
        land(&path, holes);
        panic!(
            "no baseline: wrote {} hole(s) across {scanned} {unit}. Read it \
             before committing — every line is {line_is}.",
            holes.len(),
        );
    };
    assert!(verdict.is_clean(), "{}", verdict.report(family));
}
