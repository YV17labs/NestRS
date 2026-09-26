//! Variables set under the framework's prefix that no config claims.
//!
//! A deployment that misspells a variable, or keeps a name a release renamed,
//! gets the default and no signal: nothing ever asks for the value, so nothing
//! can say it was ignored. This is where that silence ends. Two shapes are
//! reported, at `warn`, once per variable, by name and never by value:
//!
//! - **An unread key** — [`UNREAD_CONFIG_VARIABLE`]. The variable sits under a
//!   namespace a config of this binary read, and names a key that nothing read.
//!   The closest key that *was* read comes back as `suggestion` when one is
//!   near enough to be the intended one.
//! - **A misspelled namespace** — [`MISSPELLED_CONFIG_NAMESPACE`]. The namespace
//!   the variable spells is none this binary links, but equals one once the
//!   separators are set aside: `OAUTH_RESOURCE` for `oauth__resource`, the
//!   family-level rename 7.0 made. The linked spelling comes back as
//!   `suggestion`.
//!
//! **Everything else is silent, and that is the design rather than a gap.** One
//! `.env` routinely serves several binaries — the demo's `api` and `worker` link
//! different configs — so a namespace this binary does not know is another
//! binary's, never a mistake. The same reading makes a key holding `__` under a
//! known namespace a sub-namespace some other binary links (`redis__worker`
//! under `redis`), reported only when it is a near miss of a key read here.
//! A name without `__` after the prefix is outside the namespaced grammar
//! altogether — `<PREFIX>_ENV`, `<PREFIX>_LOG*`, the prefix's own
//! `NESTRS_ENV_PREFIX`, a tool's variable — and is never examined.
//!
//! # Where it runs, and why there
//!
//! **A config's keys are knowable only where its `from_env` runs.** They reach
//! the reader from a literal, a `const`, a sub-struct's own `from_env`, or a
//! branch: `HttpCors` reads five of its six keys only when `CORS_ORIGINS` is
//! set. So the key half runs inside [`read`](crate::read) — the one funnel
//! every path into `from_env` takes — for the namespace just read, once per
//! namespace per process, and only when `from_env` returned: an early `?`
//! leaves the record partial, and a key it never reached is not a key nobody
//! reads. By then the prefix is resolved and the `.env` cascade parsed, since
//! the read consulted both, and an `App` boot has installed its subscriber.
//!
//! One process-wide pass over every linked config was the alternative, and it
//! is refused: it would have to run each `from_env` a second time against a
//! recording source — developer code that may warn or open secret files — and
//! any key behind a branch it did not take would be reported on a deployment
//! that sets it correctly. A diagnostic that fires on the correct configuration
//! teaches operators to filter the target out.
//!
//! **The namespace half needs no keys**, so it runs once per linked namespace,
//! all of them at the first read: the link-time registry
//! ([`ConfigNamespace`](crate::ConfigNamespace)) is complete from the first line
//! of `main`, and running early names a renamed *required* variable before the
//! boot error its absence causes.
//!
//! A linked config that is never read has no known keys and its variables reach
//! nothing in this binary, so it files no key report; the binary that reads it
//! does. Only a reader backed by the environment
//! ([`ConfigService::for_namespace`](crate::ConfigService::for_namespace))
//! triggers either half — a reader on a custom source says nothing about the
//! variables a deployment exported.
//!
//! **Known** means asked for by a framework reader in this process: the
//! [`ConfigService`](crate::ConfigService) funnel, which records both spellings
//! of every key (`<KEY>` and `<KEY>_FILE`), and the free
//! [`env_var`](crate::env_var). A namespace read without a `Config` behind it —
//! `nest-rs-opentelemetry`'s, which runs before the container exists — is known
//! as far as it was read, and gets no key report of its own: nothing marks the
//! moment its reads are complete.

use std::collections::BTreeSet;
use std::sync::Mutex;

use nest_rs_core::EnvPrefix;

use crate::service::var_name;

/// The event for a variable under a namespace this binary read, naming a key no
/// config read. Fields: `variable`, `namespace`, and `suggestion` when a key
/// that was read is close enough to be the intended one.
pub const UNREAD_CONFIG_VARIABLE: &str = "config variable read by no config";

/// The event for a variable whose namespace matches a linked one only once the
/// separators are set aside. Fields: `variable`, `namespace` (the linked
/// spelling), and `suggestion` — the variable under that spelling.
pub const MISSPELLED_CONFIG_NAMESPACE: &str = "config variable under a misspelled namespace";

/// A variable set under the framework's prefix that no config claims, and why.
#[derive(Debug, PartialEq, Eq)]
enum Unclaimed<'a> {
    /// Under a namespace this binary read, naming a key nothing read.
    Key {
        variable: String,
        namespace: &'a str,
        suggestion: Option<String>,
    },
    /// Under a spelling of a linked namespace that differs in its separators.
    Namespace {
        variable: String,
        namespace: &'a str,
        suggestion: String,
    },
}

impl Unclaimed<'_> {
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
                namespace = *namespace,
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
                namespace = *namespace,
                suggestion = suggestion.as_str(),
                "{MISSPELLED_CONFIG_NAMESPACE}",
            ),
        }
    }
}

/// What this process has read, checked and reported.
struct Ledger {
    /// Every name a framework reader asked for, both spellings of each key.
    known: BTreeSet<String>,
    /// Namespaces whose misspellings were looked for.
    spellings_checked: BTreeSet<String>,
    /// Namespaces whose unread keys were looked for.
    keys_checked: BTreeSet<String>,
    /// Variables already reported — the once-per-variable guarantee.
    reported: BTreeSet<String>,
}

static LEDGER: Mutex<Ledger> = Mutex::new(Ledger {
    known: BTreeSet::new(),
    spellings_checked: BTreeSet::new(),
    keys_checked: BTreeSet::new(),
    reported: BTreeSet::new(),
});

/// Record that a framework reader asked for `name`.
///
/// A poisoned ledger skips the record rather than panicking the read: a missed
/// diagnostic is a diagnostic, while a config read that aborts over the
/// bookkeeping of a warning would make the warning an outage of its own.
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
    // Listed before the ledger is locked: the cascade parse behind it is the
    // one step here that touches the filesystem.
    let names = environment_names();
    let reports = {
        let Ok(mut ledger) = LEDGER.lock() else {
            return;
        };
        let mut linked: Vec<&str> = crate::namespace::linked();
        if !linked.contains(&namespace) {
            linked.push(namespace);
        }
        let unchecked: Vec<&str> = linked
            .iter()
            .copied()
            .filter(|ns| ledger.spellings_checked.insert((*ns).to_owned()))
            .collect();
        let mut out = misspelled_namespaces(&names, &ledger.known, &linked, &unchecked);
        if complete && ledger.keys_checked.insert(namespace.to_owned()) {
            out.extend(unread_keys(namespace, &names, &ledger.known, &linked));
        }
        out.retain(|found| ledger.reported.insert(found.variable().to_owned()));
        out
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
fn environment_names() -> BTreeSet<String> {
    let root = EnvPrefix::var("");
    let mut names: BTreeSet<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| name.starts_with(&root))
        .collect();
    names.extend(
        crate::dotenv::dotenv_values()
            .keys()
            .filter(|name| name.starts_with(&root))
            .cloned(),
    );
    names
}

/// The variables under `namespace` that name a key nothing read.
///
/// A variable a longer linked namespace owns (`redis__worker` under `redis`) is
/// that namespace's to judge. A key holding `__` is a sub-namespace this binary
/// does not link — another binary's — and is reported only as a near miss.
fn unread_keys<'a>(
    namespace: &'a str,
    names: &BTreeSet<String>,
    known: &BTreeSet<String>,
    linked: &[&str],
) -> Vec<Unclaimed<'a>> {
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
                namespace,
                suggestion,
            })
        })
        .collect()
}

/// The variables whose namespace equals one of `unchecked` once the separators
/// are set aside, but is spelled otherwise.
///
/// Splits are tried at each `__`, outermost first, and the first one that
/// names a linked namespace *exactly* ends the search: that variable is
/// correctly spelled, and whatever follows is its key.
fn misspelled_namespaces<'a>(
    names: &BTreeSet<String>,
    known: &BTreeSet<String>,
    linked: &[&str],
    unchecked: &[&'a str],
) -> Vec<Unclaimed<'a>> {
    let root = EnvPrefix::var("");
    let mut out = Vec::new();
    for name in names.iter().filter(|name| !known.contains(*name)) {
        let Some(rest) = name.strip_prefix(&root) else {
            continue;
        };
        for (at, _) in rest.match_indices("__").filter(|(at, _)| *at > 0) {
            let (written, key) = (&rest[..at], &rest[at + 2..]);
            if linked.iter().any(|ns| ns.eq_ignore_ascii_case(written)) {
                break;
            }
            let squashed = squash(written);
            if let Some(namespace) = unchecked.iter().copied().find(|ns| squash(ns) == squashed) {
                out.push(Unclaimed::Namespace {
                    variable: name.clone(),
                    namespace,
                    suggestion: var_name(namespace, key),
                });
                break;
            }
        }
    }
    out
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
        assert_eq!(closest("port", &read), Some(port.as_str()), "case is folded");
        assert_eq!(closest("P_O_R_T", &read), Some(port.as_str()), "separators too");
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
        let known = set(&[var_name("fixture", "PORT"), var_name("fixture", "PORT_FILE")]);
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
                    namespace: "fixture",
                    suggestion: None,
                },
                Unclaimed::Key {
                    variable: var_name("fixture", "PROT"),
                    namespace: "fixture",
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
                namespace: "fixture",
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
        let found = misspelled_namespaces(&names, &BTreeSet::new(), &linked, &linked);
        assert_eq!(
            found,
            [
                Unclaimed::Namespace {
                    variable: var_name("fixturemember", "URL_FILE"),
                    namespace: "fixture__member",
                    suggestion: var_name("fixture__member", "URL_FILE"),
                },
                Unclaimed::Namespace {
                    variable: var_name("fixture_member", "URL"),
                    namespace: "fixture__member",
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
        let found = misspelled_namespaces(&names, &BTreeSet::new(), &linked, &linked);
        assert!(
            found.is_empty(),
            "`FIXTURE` is linked, so `MEMBER__URL` is its key, not a namespace: {found:?}",
        );
    }
}
