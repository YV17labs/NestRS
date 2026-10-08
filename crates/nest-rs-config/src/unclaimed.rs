//! Variables set under the framework's prefix that no config claims.
//!
//! Two shapes are reported, at `warn`, once per variable, by name and never by
//! value:
//!
//! - **An unread key** — [`UNREAD_CONFIG_VARIABLE`]. The variable sits under a
//!   namespace a config of this binary read, and names a key that nothing read.
//!   The closest key that *was* read comes back as `suggestion` when one is
//!   near enough to be the intended one.
//! - **A misspelled namespace** — [`MISSPELLED_CONFIG_NAMESPACE`]. The variable
//!   spells a namespace this binary read otherwise than the loader reads it —
//!   other separators (`OAUTH_RESOURCE` for `oauth__resource`, `SEAORM_URL` for
//!   `SEAORM__URL`), another case, prefix included, or one misspelled segment
//!   of six letters or more (`PORBE_KEYS` for `probe_keys`, [`is_near_miss`]) —
//!   **and the key after it is one that namespace reads**, or one edit from
//!   one. The linked spelling comes back as `suggestion`.
//!
//! Everything else is silent: one `.env` serves several binaries, so a namespace
//! this binary does not know, and no near miss of one it does, is another
//! binary's. A key holding `__` under a known namespace is a sub-namespace
//! another binary links (`redis__queue` under `redis`), reported only as a near
//! miss of a key read here.
//!
//! A name with no `__` after the prefix holds no namespace: it is reported only
//! when it equals, separators and case aside, a name this process reads — the
//! framework-wide ones (`EnvPrefix::VAR`,
//! [`Environment::var_name`](crate::Environment::var_name), the kernel's log
//! settings) included, known by declaration since no read makes them known.
//!
//! # Where it runs
//!
//! A config's keys are knowable only where its `from_env` runs, so the key half
//! runs inside [`read`](crate::read), once per namespace, and only once a
//! `from_env` in it has returned: an early `?` leaves the record partial. The
//! namespace half runs at every read once the namespace has been read. A read
//! ended by an error reports before the error ends the boot.
//!
//! A read with nothing listening at `warn` on [`TARGET`](crate::TARGET) records
//! what it read and reports nothing; the first read with a listener files what
//! the earlier ones found. Only a reader backed by the environment
//! ([`ConfigService::for_namespace`](crate::ConfigService::for_namespace))
//! triggers either half. A namespace read without a `Config` behind it
//! (`nest-rs-opentelemetry`'s) is known as far as it was read, and gets no
//! report of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use nest_rs_core::EnvPrefix;

use crate::service::var_name;

/// The event for a variable under a namespace this binary read, naming a key no
/// config read. Fields: `variable`, `namespace`, and `suggestion` when a key
/// that was read is close enough to be the intended one.
pub const UNREAD_CONFIG_VARIABLE: &str = "config variable read by no config";

/// The event for a variable whose namespace is a near miss of a linked one —
/// other separators, another case, or one misspelled segment. Fields:
/// `variable`, `namespace` (the linked spelling), and `suggestion` — the
/// variable under that spelling.
pub const MISSPELLED_CONFIG_NAMESPACE: &str = "config variable under a misspelled namespace";

/// A variable set under the framework's prefix that no config claims, and why.
#[derive(Debug, PartialEq, Eq)]
enum Unclaimed {
    /// Under a namespace this binary read, naming a key nothing read.
    Key {
        variable: String,
        namespace: String,
        suggestion: Option<String>,
    },
    /// Under a near miss of a linked namespace — its separators, the `__` that
    /// ends it included, its case, or one of its segments.
    Namespace {
        variable: String,
        namespace: String,
        suggestion: String,
    },
}

impl Unclaimed {
    fn variable(&self) -> &str {
        match self {
            Self::Key { variable, .. } | Self::Namespace { variable, .. } => variable,
        }
    }

    /// File the event. Names only: the value is never read, so it cannot leak.
    fn report(&self) {
        match self {
            Self::Key {
                variable,
                namespace,
                suggestion,
            } => tracing::warn!(
                target: crate::TARGET,
                variable = variable.as_str(),
                namespace = namespace.as_str(),
                suggestion = suggestion.as_deref(),
                "{UNREAD_CONFIG_VARIABLE}",
            ),
            Self::Namespace {
                variable,
                namespace,
                suggestion,
            } => tracing::warn!(
                target: crate::TARGET,
                variable = variable.as_str(),
                namespace = namespace.as_str(),
                suggestion = suggestion.as_str(),
                "{MISSPELLED_CONFIG_NAMESPACE}",
            ),
        }
    }
}

/// What this process has read, checked and reported.
struct Ledger {
    /// Every name a framework reader asked for, both spellings of each key,
    /// and the framework-wide names read outside the funnel ([`framework_wide`]).
    known: BTreeSet<String>,
    /// Namespaces a config was read in, whether or not the read finished —
    /// each owns the variables under it, linked or not.
    read: BTreeSet<String>,
    /// Namespaces in which a `from_env` returned, so every key it reads is known.
    complete: BTreeSet<String>,
    /// Namespaces whose unread keys were looked for.
    keys_checked: BTreeSet<String>,
    /// Variables already reported — the once-per-variable guarantee.
    reported: BTreeSet<String>,
}

impl Ledger {
    const fn new() -> Self {
        Self {
            known: BTreeSet::new(),
            read: BTreeSet::new(),
            complete: BTreeSet::new(),
            keys_checked: BTreeSet::new(),
            reported: BTreeSet::new(),
        }
    }

    /// Note that a config in `namespace` was read, `complete` when its
    /// `from_env` returned.
    fn record_read(&mut self, namespace: &str, complete: bool) {
        self.read.insert(namespace.to_owned());
        if complete {
            self.complete.insert(namespace.to_owned());
        }
    }

    /// What `names` holds that no config claims, in a binary linking `linked`,
    /// given the reads recorded so far.
    ///
    /// Each namespace's keys are checked once, after a read that finished; its
    /// misspellings at every check, against the keys known by then. A variable
    /// two checks both see is still reported once.
    fn check(&mut self, names: &BTreeSet<String>, linked: &[&str]) -> Vec<Unclaimed> {
        self.known.extend(framework_wide());
        let owners: BTreeSet<&str> = linked
            .iter()
            .copied()
            .chain(self.read.iter().map(String::as_str))
            .collect();
        let owners: Vec<&str> = owners.into_iter().collect();
        let read: Vec<&str> = self.read.iter().map(String::as_str).collect();
        let keys: Vec<&str> = self
            .complete
            .iter()
            .map(String::as_str)
            .filter(|ns| !self.keys_checked.contains(*ns))
            .collect();

        let mut out = misspelled_namespaces(names, &self.known, &owners, &read);
        for namespace in &keys {
            out.extend(unread_keys(namespace, names, &self.known, &owners));
        }
        out.extend(run_together(names, &self.known, &owners));

        let keys: Vec<String> = keys.iter().map(|ns| (*ns).to_owned()).collect();
        self.keys_checked.extend(keys);
        out.retain(|found| self.reported.insert(found.variable().to_owned()));
        out
    }
}

/// The framework's variables no config reads, named by the constants that
/// declare them: the prefix's bootstrap variable, the environment selector and
/// the kernel's log settings.
///
/// Known by declaration because no read makes them known: the kernel reads its
/// log settings before this crate is reached, and the selector is read to pick
/// the cascade every later read consults.
fn framework_wide() -> [String; 5] {
    use nest_rs_core::logging::var;
    [
        EnvPrefix::VAR.to_owned(),
        crate::Environment::var_name(),
        EnvPrefix::var(var::FILTER),
        EnvPrefix::var(var::FORMAT),
        EnvPrefix::var(var::SOURCE_LOCATION),
    ]
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger::new());

/// Record that a framework reader asked for `name`.
///
/// A poisoned ledger skips the record rather than failing the read.
pub(crate) fn witness(name: &str) {
    let Ok(mut ledger) = LEDGER.lock() else {
        return;
    };
    if !ledger.known.contains(name) {
        ledger.known.insert(name.to_owned());
    }
}

/// Report what the environment sets that no config claims, now that the config
/// in `namespace` has been read. `complete` is whether its `from_env` returned —
/// a partial read knows only part of its keys, so it reports misspelled
/// namespaces and leaves its keys for a read that finishes.
pub(crate) fn after_read(namespace: &str, complete: bool) {
    match LEDGER.lock() {
        Ok(mut ledger) => ledger.record_read(namespace, complete),
        Err(_) => return,
    }
    // Nothing would hear the report: the read stays recorded, and the first
    // read that has a listener files what this one found.
    if !tracing::enabled!(target: crate::TARGET, tracing::Level::WARN) {
        return;
    }
    // Gathered before the ledger is locked again: the cascade parse behind the
    // names is the one step here that touches the filesystem.
    let names = environment_names();
    let linked = crate::namespace::linked();
    let reports = match LEDGER.lock() {
        Ok(mut ledger) => ledger.check(&names, &linked),
        Err(_) => return,
    };
    // Emitted with the ledger released: a subscriber is foreign code, and one
    // that reads a config would otherwise wait on a lock its own thread holds.
    for found in &reports {
        found.report();
    }
}

/// Every variable name under the prefix, from the process environment and the
/// `.env` cascade — the two tiers a config read consults. Values are dropped as
/// they are listed.
///
/// A name whose prefix is written in another case is listed too: the loader
/// never reads it, so it is a misspelling to report. Windows is the exception,
/// for the process environment only — its lookups fold case, so a name there is
/// listed as the loader reads it.
fn environment_names() -> BTreeSet<String> {
    let root = EnvPrefix::var("");
    let under_root = |name: &String| strip_root(name, &root).is_some();
    let mut names: BTreeSet<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .map(|name| {
            if cfg!(windows) {
                name.to_ascii_uppercase()
            } else {
                name
            }
        })
        .filter(under_root)
        .collect();
    names.extend(
        crate::dotenv::dotenv_values()
            .keys()
            .filter(|name| under_root(name))
            .cloned(),
    );
    names
}

/// The variables under `namespace` that name a key nothing read.
///
/// A variable a longer linked namespace owns (`redis__queue` under `redis`) is
/// that namespace's to judge. A key holding `__` is a sub-namespace this binary
/// does not link — another binary's — and is reported only as a near miss.
fn unread_keys(
    namespace: &str,
    names: &BTreeSet<String>,
    known: &BTreeSet<String>,
    linked: &[&str],
) -> Vec<Unclaimed> {
    let head = var_name(namespace, "");
    let deeper: Vec<String> = linked
        .iter()
        .filter(|other| other.len() > namespace.len())
        .map(|other| var_name(other, ""))
        .filter(|other| other.starts_with(&head))
        .collect();
    let read: Vec<(&str, &str)> = known
        .iter()
        .filter_map(|name| Some((name.strip_prefix(&head)?, name.as_str())))
        .collect();
    names
        .iter()
        .filter(|name| !known.contains(*name))
        .filter(|name| !deeper.iter().any(|other| name.starts_with(other.as_str())))
        .filter_map(|name| {
            let key = name.strip_prefix(&head)?;
            let suggestion = closest(key, &read).map(ToOwned::to_owned);
            if key.contains("__") && suggestion.is_none() {
                return None;
            }
            Some(Unclaimed::Key {
                variable: name.clone(),
                namespace: namespace.to_owned(),
                suggestion,
            })
        })
        .collect()
}

/// The variables whose namespace is a near miss of one of `read` — equal once
/// separators and case are set aside, or one misspelled segment away from it
/// ([`is_near_miss`]) — **and whose key is one that namespace reads**, or one
/// edit from one.
///
/// The key half keeps another binary's variable silent: `openai` is one edit
/// from `openapi`, but `OPENAI__API_KEY` names no key `openapi` reads.
///
/// The loader is exact, so a namespace is spelled correctly only as that exact
/// text, under the exact prefix. The longest split
/// at a `__` that names an owned namespace exactly makes the variable that
/// namespace's, and whatever follows is its key: the key half judges it —
/// unless a longer split is a near miss of a member of that namespace's own
/// family, so a typo in a member's segment (`PROBE__MEMBR` for
/// `probe__member`) is found although `probe` itself is owned. A variable with
/// no exact owner is looked for at every split. The nearest candidate wins,
/// ties to the one that sorts first.
fn misspelled_namespaces(
    names: &BTreeSet<String>,
    known: &BTreeSet<String>,
    owners: &[&str],
    read: &[&str],
) -> Vec<Unclaimed> {
    let root = EnvPrefix::var("");
    let mut out = Vec::new();
    for name in names.iter().filter(|name| !known.contains(*name)) {
        let Some((exact_prefix, rest)) = strip_root(name, &root) else {
            continue;
        };
        let splits: Vec<usize> = rest
            .match_indices("__")
            .map(|(at, _)| at)
            .filter(|at| *at > 0)
            .collect();
        let owned = exact_prefix
            .then(|| {
                splits.iter().copied().rev().find(|at| {
                    owners
                        .iter()
                        .any(|ns| ns.to_ascii_uppercase() == rest[..*at])
                })
            })
            .flatten();
        let family = owned.map(|at| format!("{}__", &rest[..at]));
        let nearest = splits
            .iter()
            .copied()
            .filter(|at| owned.is_none_or(|owned| *at > owned))
            .flat_map(|at| {
                let written = &rest[..at];
                let key = &rest[at + 2..];
                let family = family.clone();
                read.iter()
                    .copied()
                    .filter(move |ns| ns.to_ascii_uppercase() != written || !exact_prefix)
                    .filter(move |ns| {
                        family
                            .as_ref()
                            .is_none_or(|family| ns.to_ascii_uppercase().starts_with(family))
                    })
                    .filter(move |ns| reads_key(known, ns, key))
                    .filter_map(move |ns| near_miss(written, ns).map(|distance| (distance, ns, at)))
            })
            .min_by_key(|(distance, ns, _)| (*distance, *ns));
        if let Some((_, namespace, at)) = nearest {
            out.push(Unclaimed::Namespace {
                variable: name.clone(),
                namespace: namespace.to_owned(),
                suggestion: var_name(namespace, &rest[at + 2..]),
            });
        }
    }
    out
}

/// Whether `namespace` reads `key`, or a key one edit from it — separators and
/// case set aside — among the names `known` holds.
fn reads_key(known: &BTreeSet<String>, namespace: &str, key: &str) -> bool {
    let head = var_name(namespace, "");
    let typed = key.to_ascii_uppercase();
    let squashed = squash(&typed);
    known
        .iter()
        .filter_map(|name| name.strip_prefix(&head))
        .any(|read| squash(read) == squashed || distance(&typed, read) <= 1)
}

/// `name` with the prefix removed, and whether the prefix was written exactly —
/// a prefix in another case is a misspelling, never another binary's.
fn strip_root<'n>(name: &'n str, root: &str) -> Option<(bool, &'n str)> {
    let head = name.get(..root.len())?;
    head.eq_ignore_ascii_case(root)
        .then(|| (head == root, &name[root.len()..]))
}

/// The shortest segment a one-edit typo is looked for in. Under six letters a
/// namespace has no room for a typo that is not also another word — `auth` and
/// `authz` beside `authn`, `es` beside `ws`, `reds` beside `redis` — so a short
/// segment is a near miss only through its separators and its case.
const TYPO_REACH_MIN_LEN: usize = 6;

/// Whether `written` is a near miss of the namespace `namespace` — the
/// namespace half of the misspelled-namespace report, for a suite to hold a
/// tree's own namespaces apart by.
///
/// Equal once separators and case are set aside is nearest of all. Otherwise
/// the two must have as many `__` segments, all equal but one — separators and
/// case set aside — and that one, at least `TYPO_REACH_MIN_LEN` letters as
/// linked, within a quarter of its longer spelling, at least one edit:
/// `PORBE_KEYS` for `probe_keys`, `PROBE__MEMBR` for `probe__member`.
/// Tighter than a key's reach: `social__github` and `social__gitlab` must not
/// read as one misspelled for the other.
pub fn is_near_miss(written: &str, namespace: &str) -> bool {
    near_miss(written, namespace).is_some()
}

fn near_miss(written: &str, namespace: &str) -> Option<usize> {
    if squash(written) == squash(namespace) {
        return Some(0);
    }
    let (typed, linked): (Vec<String>, Vec<String>) = (
        written.split("__").map(squash).collect(),
        namespace.split("__").map(squash).collect(),
    );
    if typed.len() != linked.len() {
        return None;
    }
    let mut differing = typed.iter().zip(&linked).filter(|(a, b)| a != b);
    let (a, b) = differing.next()?;
    if differing.next().is_some() || b.chars().count() < TYPO_REACH_MIN_LEN {
        return None;
    }
    let distance = distance(a, b);
    let reach = (a.chars().count().max(b.chars().count()) / 4).max(1);
    (distance <= reach).then_some(distance)
}

/// The variables with no `__` after the prefix that equal, once separators and
/// case are set aside, a name read under a namespace this binary owns —
/// `SEAORM_URL` for `SEAORM__URL`.
///
/// Such a name is never another binary's namespaced variable, so the
/// suggestion is a name this binary reads; one equal to nothing read is left alone.
fn run_together(
    names: &BTreeSet<String>,
    known: &BTreeSet<String>,
    owners: &[&str],
) -> Vec<Unclaimed> {
    let root = EnvPrefix::var("");
    let heads: Vec<(String, &str)> = owners.iter().map(|ns| (var_name(ns, ""), *ns)).collect();
    let mut read: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for name in known {
        let owner = heads
            .iter()
            .filter(|(head, _)| name.starts_with(head.as_str()))
            .max_by_key(|(head, _)| head.len());
        if let (Some((_, namespace)), Some(rest)) = (owner, name.strip_prefix(&root)) {
            read.entry(squash(rest))
                .or_insert((namespace, name.as_str()));
        }
    }
    names
        .iter()
        .filter(|name| !known.contains(*name))
        .filter_map(|name| {
            let (_, rest) = strip_root(name, &root).filter(|(_, rest)| !rest.contains("__"))?;
            let (namespace, suggestion) = read.get(&squash(rest))?;
            Some(Unclaimed::Namespace {
                variable: name.clone(),
                namespace: (*namespace).to_owned(),
                suggestion: (*suggestion).to_owned(),
            })
        })
        .collect()
}

/// `name` with its separators removed and its case folded — the form in which
/// `OAUTH_RESOURCE`, `OAUTH__RESOURCE` and `oauth__resource` are one namespace.
fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// The known name whose key `key` most plausibly meant, if any is near enough.
///
/// Equal once separators and case are set aside is nearest of all; otherwise
/// the edit distance must stay within a third of the longer key, and at least
/// one edit — `PROT` reaches `PORT`, `HOST` does not. Ties go to the name that
/// sorts first, so the suggestion is stable across runs.
fn closest<'k>(key: &str, read: &[(&str, &'k str)]) -> Option<&'k str> {
    let typed = key.to_ascii_uppercase();
    let squashed = squash(&typed);
    read.iter()
        .filter_map(|(candidate, name)| {
            let distance = if squash(candidate) == squashed {
                0
            } else {
                distance(&typed, candidate)
            };
            let reach = (typed.chars().count().max(candidate.chars().count()) / 3).max(1);
            (distance <= reach).then_some((distance, *name))
        })
        .min()
        .map(|(_, name)| name)
}

/// Optimal-string-alignment distance: insertions, deletions, substitutions and
/// adjacent transpositions, each one edit — a swapped pair is the commonest
/// typo, and plain Levenshtein charges it two.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let width = b.len() + 1;
    let mut table = vec![0usize; (a.len() + 1) * width];
    for (col, cell) in table.iter_mut().take(width).enumerate() {
        *cell = col;
    }
    for row in 1..=a.len() {
        table[row * width] = row;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let substitution = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (table[(i - 1) * width + j] + 1)
                .min(table[i * width + j - 1] + 1)
                .min(table[(i - 1) * width + j - 1] + substitution);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(table[(i - 2) * width + j - 2] + 1);
            }
            table[i * width + j] = best;
        }
    }
    table[a.len() * width + b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[String]) -> BTreeSet<String> {
        names.iter().cloned().collect()
    }

    #[test]
    fn a_swapped_pair_is_one_edit() {
        assert_eq!(distance("PROT", "PORT"), 1);
        assert_eq!(distance("PORT", "PORT"), 0);
        assert_eq!(distance("URL", "URLS"), 1);
        assert_eq!(distance("HOST", "PORT"), 2);
        assert_eq!(distance("", "PORT"), 4);
    }

    #[test]
    fn a_near_key_is_suggested_and_a_far_one_is_not() {
        let port = var_name("fixture", "PORT");
        let host = var_name("fixture", "HOST");
        let read = [("PORT", port.as_str()), ("HOST", host.as_str())];
        assert_eq!(closest("PROT", &read), Some(port.as_str()));
        assert_eq!(
            closest("port", &read),
            Some(port.as_str()),
            "case is folded"
        );
        assert_eq!(
            closest("P_O_R_T", &read),
            Some(port.as_str()),
            "separators too"
        );
        assert_eq!(closest("TIMEOUT", &read), None);
    }

    #[test]
    fn a_renamed_suffix_is_near_enough_to_suggest() {
        let secs = var_name("fixture", "CONNECT_TIMEOUT_SECS");
        let read = [("CONNECT_TIMEOUT_SECS", secs.as_str())];
        assert_eq!(closest("CONNECT_TIMEOUT", &read), Some(secs.as_str()));
    }

    #[test]
    fn a_key_nothing_read_is_reported_with_its_nearest_neighbour() {
        let known = set(&[
            var_name("fixture", "PORT"),
            var_name("fixture", "PORT_FILE"),
        ]);
        let names = set(&[
            var_name("fixture", "PROT"),
            var_name("fixture", "BANNER"),
            var_name("fixture", "PORT"),
        ]);
        let found = unread_keys("fixture", &names, &known, &["fixture"]);
        assert_eq!(
            found,
            [
                Unclaimed::Key {
                    variable: var_name("fixture", "BANNER"),
                    namespace: "fixture".to_owned(),
                    suggestion: None,
                },
                Unclaimed::Key {
                    variable: var_name("fixture", "PROT"),
                    namespace: "fixture".to_owned(),
                    suggestion: Some(var_name("fixture", "PORT")),
                },
            ],
        );
    }

    /// `MEMBER__URL` is a near miss of the `MEMBER_URL` key `fixture` read, so
    /// only the ownership of the longer namespace keeps it out of `fixture`'s
    /// report.
    #[test]
    fn a_longer_linked_namespace_owns_its_own_variables() {
        let known = set(&[var_name("fixture", "MEMBER_URL")]);
        let names = set(&[var_name("fixture__member", "URL")]);
        let found = unread_keys("fixture", &names, &known, &["fixture", "fixture__member"]);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_sub_namespace_this_binary_does_not_link_is_another_binarys() {
        let known = set(&[var_name("fixture", "CLIENT_ID")]);
        let names = set(&[
            var_name("fixture__elsewhere", "TOKEN"),
            var_name("fixture", "CLIENT__ID"),
        ]);
        let found = unread_keys("fixture", &names, &known, &["fixture"]);
        assert_eq!(
            found,
            [Unclaimed::Key {
                variable: var_name("fixture", "CLIENT__ID"),
                namespace: "fixture".to_owned(),
                suggestion: Some(var_name("fixture", "CLIENT_ID")),
            }],
            "only the near miss is reported",
        );
    }

    #[test]
    fn a_namespace_spelled_with_other_separators_is_reported_under_its_linked_spelling() {
        let names = set(&[
            var_name("fixture_member", "URL"),
            var_name("fixturemember", "URL_FILE"),
            var_name("fixture__member", "URL"),
            var_name("fixture__other", "URL"),
            var_name("unrelated", "URL"),
        ]);
        let linked = ["fixture__member"];
        let known = set(&[
            var_name("fixture__member", "URL"),
            var_name("fixture__member", "URL_FILE"),
        ]);
        let found = misspelled_namespaces(&names, &known, &linked, &linked);
        assert_eq!(
            found,
            [
                Unclaimed::Namespace {
                    variable: var_name("fixturemember", "URL_FILE"),
                    namespace: "fixture__member".to_owned(),
                    suggestion: var_name("fixture__member", "URL_FILE"),
                },
                Unclaimed::Namespace {
                    variable: var_name("fixture_member", "URL"),
                    namespace: "fixture__member".to_owned(),
                    suggestion: var_name("fixture__member", "URL"),
                },
            ],
        );
    }

    #[test]
    fn a_name_some_reader_asked_for_is_never_misspelled() {
        let legacy = var_name("fixture_member", "URL");
        let found = misspelled_namespaces(
            &set(std::slice::from_ref(&legacy)),
            &set(std::slice::from_ref(&legacy)),
            &["fixture__member"],
            &["fixture__member"],
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_correctly_spelled_namespace_ends_the_search_before_its_keys_are_read_as_one() {
        let names = set(&[var_name("fixture", "MEMBER__URL")]);
        let linked = ["fixture", "fixturemember"];
        let known = set(&[var_name("fixturemember", "URL")]);
        let found = misspelled_namespaces(&names, &known, &linked, &linked);
        assert!(
            found.is_empty(),
            "`FIXTURE` is linked, so `MEMBER__URL` is its key, not a namespace: {found:?}",
        );
    }

    /// The loader is exact: a namespace spelled in another case is not the
    /// linked one, however its letters compare once case is folded.
    #[test]
    fn a_namespace_in_another_case_is_misspelled() {
        let written = var_name("fixture", "PORT").replace("FIXTURE", "Fixture");
        let found = misspelled_namespaces(
            &set(std::slice::from_ref(&written)),
            &set(&[var_name("fixture", "PORT")]),
            &["fixture"],
            &["fixture"],
        );
        assert_eq!(
            found,
            [Unclaimed::Namespace {
                variable: written,
                namespace: "fixture".to_owned(),
                suggestion: var_name("fixture", "PORT"),
            }],
        );
    }

    /// A typo in a member's own segment is found although the family's root is
    /// owned too — and only a member of that root's family is a candidate there.
    #[test]
    fn a_typo_in_a_member_segment_is_found_under_an_owned_root() {
        let typo = var_name("fixture__membr", "URL");
        let linked = ["fixture", "fixture__member", "fixturemembr"];
        let known = set(&[
            var_name("fixture__member", "URL"),
            var_name("fixturemembr", "URL"),
        ]);
        let found =
            misspelled_namespaces(&set(std::slice::from_ref(&typo)), &known, &linked, &linked);
        assert_eq!(
            found,
            [Unclaimed::Namespace {
                variable: typo,
                namespace: "fixture__member".to_owned(),
                suggestion: var_name("fixture__member", "URL"),
            }],
        );
    }

    #[test]
    fn a_near_miss_is_one_misspelled_segment_within_a_quarter_of_it() {
        assert!(is_near_miss("PORBE_KEYS", "probe_keys"), "a swapped pair");
        assert!(
            is_near_miss("PROBE__MEMBR", "probe__member"),
            "a member's segment"
        );
        assert!(
            is_near_miss("OAUTH_RESOURCE", "oauth__resource"),
            "separators alone"
        );
        assert!(
            !is_near_miss("SOCIAL__GITLAB", "social__github"),
            "a sibling member"
        );
        assert!(
            !is_near_miss("FIXTURE__OTHER", "fixture__member"),
            "another word"
        );
        assert!(
            !is_near_miss("PROBE__MEMBR", "probe__membr__x"),
            "another depth"
        );
        assert!(
            !is_near_miss("PORBE__MEMBR", "probe__member"),
            "two misspelled segments"
        );
        assert!(is_near_miss("SEAROM", "seaorm"), "six letters hold a typo");
    }

    /// A segment under six letters is a near miss only through its separators
    /// and its case.
    #[test]
    fn a_short_segment_is_a_near_miss_only_by_its_separators_and_case() {
        for (written, linked) in [
            ("AUTH", "authn"),
            ("AUTHZ", "authn"),
            ("ES", "ws"),
            ("REDS", "redis"),
            ("HTTPS", "http"),
            ("OAUTHS__CLIENT", "oauth__client"),
        ] {
            assert!(
                !is_near_miss(written, linked),
                "{written} is a word beside {linked}"
            );
        }
        assert!(is_near_miss("AUTH_N", "authn"), "separators set aside");
        assert!(is_near_miss("Redis", "redis"), "case set aside");
    }

    /// A near miss of a namespace is reported only under a key that namespace
    /// reads, or one edit from one.
    #[test]
    fn a_near_namespace_is_reported_only_under_a_key_it_reads() {
        let known = set(&[
            var_name("openapi", "TITLE"),
            var_name("seaorm", "URL"),
            var_name("seaorm", "URL_FILE"),
        ]);
        let openai = var_name("openai", "API_KEY");
        let typo = var_name("openai", "TITLE");
        let both = var_name("openai", "TITEL");
        let searom = var_name("searom", "URL");
        let names = set(&[openai.clone(), typo.clone(), both.clone(), searom.clone()]);
        let read = ["openapi", "seaorm"];
        let found = misspelled_namespaces(&names, &known, &read, &read);
        let reported: Vec<&str> = found.iter().map(Unclaimed::variable).collect();
        assert_eq!(reported, [both.as_str(), typo.as_str(), searom.as_str()]);
    }

    /// A namespace read before a longer one this binary links only by a
    /// hand-written `Namespaced` sees that one's variable as a near miss of its
    /// own key; the longer one's own read sees it as unread. One line, not two.
    #[test]
    fn a_variable_two_checks_can_see_is_reported_once() {
        let mut ledger = Ledger::new();
        ledger.known.insert(var_name("fixture", "MEMBER_URL"));
        let names = set(&[var_name("fixture__member", "URL")]);

        ledger.record_read("fixture", true);
        let first = ledger.check(&names, &[]);
        ledger.record_read("fixture__member", true);
        let second = ledger.check(&names, &[]);

        assert_eq!(first.len(), 1, "{first:?}");
        assert!(second.is_empty(), "{second:?}");
    }

    /// A read cut short by an early `?` knows only part of its keys: it reports
    /// no key, and leaves the namespace for a read that finishes.
    #[test]
    fn a_partial_read_leaves_its_keys_for_a_complete_one() {
        let mut ledger = Ledger::new();
        let names = set(&[var_name("fixture", "PROT")]);

        ledger.record_read("fixture", false);
        assert!(ledger.check(&names, &[]).is_empty());
        ledger.known.insert(var_name("fixture", "PORT"));
        ledger.record_read("fixture", true);
        let found = ledger.check(&names, &[]);

        assert_eq!(
            found,
            [Unclaimed::Key {
                variable: var_name("fixture", "PROT"),
                namespace: "fixture".to_owned(),
                suggestion: Some(var_name("fixture", "PORT")),
            }],
        );
    }

    /// A read with nothing listening records itself and checks nothing; the
    /// next check covers its keys as well as its own.
    #[test]
    fn a_read_nobody_heard_is_checked_by_the_next_one() {
        let mut ledger = Ledger::new();
        ledger
            .known
            .extend([var_name("fixture", "PORT"), var_name("other", "URL")]);
        let names = set(&[var_name("fixture", "PROT"), var_name("other", "URLL")]);

        ledger.record_read("fixture", true);
        ledger.record_read("other", true);
        let found = ledger.check(&names, &[]);

        let reported: Vec<&str> = found.iter().map(Unclaimed::variable).collect();
        assert_eq!(
            reported,
            [var_name("fixture", "PROT"), var_name("other", "URLL")],
        );
    }

    /// The level separator run into a word one — at the end of the namespace
    /// and inside it — is answered with the name that is read, `_FILE` spelling
    /// included; a name read under no namespace this binary owns is no answer.
    #[test]
    fn a_run_together_name_is_answered_with_the_name_read() {
        let url = var_name("fixture__member", "URL");
        let file = var_name("fixture__member", "URL_FILE");
        let borrowed = var_name("borrowed", "URL");
        let known = set(&[url.clone(), file.clone(), borrowed.clone()]);
        let names = set(&[
            url.replace("__", "_"),
            file.replace("__", "_"),
            borrowed.replace("__", "_"),
            var_name("unrelated", "URL").replace("__", "_"),
        ]);

        let found = run_together(&names, &known, &["fixture__member"]);

        assert_eq!(
            found,
            [
                Unclaimed::Namespace {
                    variable: url.replace("__", "_"),
                    namespace: "fixture__member".to_owned(),
                    suggestion: url,
                },
                Unclaimed::Namespace {
                    variable: file.replace("__", "_"),
                    namespace: "fixture__member".to_owned(),
                    suggestion: file,
                },
            ],
        );
    }

    /// `<PREFIX>_LOG_FORMAT` equals `<PREFIX>_LOG__FORMAT` once separators are
    /// set aside, so a binary whose own `log` namespace reads `FORMAT` would
    /// answer the kernel's variable with its own — unless the kernel's is known.
    #[test]
    fn a_framework_wide_name_is_known_where_a_namespace_would_claim_it() {
        let mut ledger = Ledger::new();
        ledger
            .known
            .extend([var_name("log", "FORMAT"), var_name("log", "LEVEL")]);
        ledger.record_read("log", true);
        let kernel = EnvPrefix::var(nest_rs_core::logging::var::FORMAT);
        let control = var_name("log", "LEVEL").replace("__", "_");

        let found = ledger.check(&set(&[kernel, control.clone()]), &["log"]);

        assert_eq!(
            found,
            [Unclaimed::Namespace {
                variable: control,
                namespace: "log".to_owned(),
                suggestion: var_name("log", "LEVEL"),
            }],
        );
    }
}
