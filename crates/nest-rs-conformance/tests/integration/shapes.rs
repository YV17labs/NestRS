//! The shapes join: every decorator pair, against the wrong shapes it owes a
//! named refusal for.
//!
//! `CLAUDE.md`'s *No decorator on two item shapes* states the testable form —
//! "both halves parse through one `DecoratorPair` const" and "each pair ships a
//! trybuild snapshot **per** wrong shape". The first half is checked by a grep
//! a human runs; the second was checked by nobody, and the missing column was
//! the same at eight sites: **an impl half posted on `impl Trait for T` is
//! accepted in silence and expands to an empty collection**, so a route, a
//! message or a tick declared there disappears without a word.
//!
//! Members come from the `DecoratorPair` declarations themselves — the
//! authority the rule names — so a pair written next year owes its cells the
//! day it is written. The join key is the fixture's file name, which the repo
//! already spells `<decorator>_<wrong shape>`; nothing here invents a
//! convention, it reads the one in use.
//!
//! **The cell reads the snapshot's *sentences*, not its name and not its
//! frame.** The rule's words are "a compile error **naming the sibling**, never
//! `expected struct`", and a file name asserts nothing about what is in it —
//! the shape `edges.rs` records having corrected in itself, where
//! `"authorize.rs".is_file()` closed a cell for a day.
//!
//! Reading the whole `.stderr` was the same defect one layer in, and it shipped
//! here for a round: trybuild writes the fixture's own path into every snapshot
//! (`--> tests/integration/diagnostics/routes_on_a_trait_impl.rs:11:6`), so
//! `contains("routes")` was satisfied by the file name a second time, through
//! the frame rather than through the name. Stripping the source frame — the
//! `-->` locator, the gutter and the underline — leaves the `error:` /
//! `note:` / `help:` prose, which is the only part a wording regression can
//! change. A snapshot re-blessed with `TRYBUILD=overwrite` after such a
//! regression now fails the cell.

use std::collections::{BTreeMap, BTreeSet};

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{Pair, declared_pairs, repo_root, rust_files};

const BASELINE: &str = "shapes-baseline.txt";

/// Nine pairs stand today: four edges plus five whose struct half is the
/// generic `#[injectable]`. Below that the scan is reading the wrong tree.
const FLOOR: usize = 9;

/// `#[controller]` → `controller`, which is how the fixtures spell it.
fn bare(decorator: &str) -> String {
    decorator
        .trim_start_matches("#[")
        .trim_end_matches(']')
        .to_owned()
}

/// Every trybuild fixture in the workspace, by bare file name, with the snapshot
/// it is pinned against. The population is workspace-wide because a pair's
/// refusal is proved in its *surface* crate, never in the macro crate that emits
/// it.
///
/// A fixture with no `.stderr` maps to an empty string, which fails every
/// naming check below — a compile-fail case nothing pins is not a refusal.
fn fixtures() -> BTreeMap<String, String> {
    let root = repo_root();
    rust_files(&root.join("crates"))
        .into_iter()
        .filter(|p| p.to_string_lossy().contains("/diagnostics/"))
        .filter_map(|p| {
            let stem = p.file_stem().and_then(|s| s.to_str())?.to_owned();
            let snapshot = std::fs::read_to_string(p.with_extension("stderr")).unwrap_or_default();
            Some((stem, diagnostic_prose(&snapshot)))
        })
        .collect()
}

/// The sentences a compiler diagnostic carries, with the source frame removed.
///
/// rustc's frame is `--> path:line:col`, a ` | ` gutter and the underline
/// beneath it. All three quote the *fixture*, so all three carry its file name
/// — which is the decorator's name, which is what the cell is trying to prove
/// the sentence says. Keeping only the lines that open a diagnostic
/// (`error:` / `warning:` / `note:` / `help:`, at any indent) leaves exactly the
/// prose a wording regression rewrites.
fn diagnostic_prose(snapshot: &str) -> String {
    snapshot
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            ["error", "warning", "note", "help"].iter().any(|kind| {
                line.starts_with(&format!("{kind}:")) || line.starts_with(&format!("{kind}["))
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a pair owes, as fixture names — every column the repo already spells
/// for at least one pair. A convention held by one member and missing from the
/// other eight is the shape of every hole here.
///
/// Four cells for any pair:
///
/// - the host half on an `impl` — the shape that answers `expected struct`
///   instead of naming its sibling;
/// - the impl half on a `struct`;
/// - the impl half on a **trait** impl, which parses as an `impl` all the same;
/// - the impl half carrying arguments, which it never takes.
///
/// Four more when the struct half is the generic `#[injectable]`: the host is
/// then a provider, and `Container::get::<Host>()` answers only for a singleton
/// held under its own type. `framework.md` refuses that at four sites, and
/// `#[hooks]` is the only pair whose fixtures pin all four.
fn owed(pair: &Pair) -> Vec<(String, Option<String>)> {
    let host = bare(&pair.host);
    let ops = bare(&pair.operations);
    // The two cross-shape cells carry the name the refusal must spell — the
    // *other* half of the pair, which is the whole content of "naming the
    // sibling". The other two are about the decorator's own grammar, so they
    // owe their own name and nothing further.
    let mut cells = vec![
        (format!("{host}_on_impl"), Some(ops.clone())),
        (format!("{ops}_on_struct"), Some(host.clone())),
        (format!("{ops}_on_a_trait_impl"), Some(ops.clone())),
        (format!("{ops}_takes_no_arguments"), Some(ops.clone())),
    ];
    if pair.host == "#[injectable]" {
        // The residency refusals name a *fact* about the provider rather than a
        // sibling, so the snapshot is required to exist and not to spell a
        // particular word.
        cells.extend([
            (format!("{ops}_on_a_non_provider"), None),
            (format!("{ops}_on_a_request_scoped_provider"), None),
            (format!("{ops}_on_a_transient_provider"), None),
            (format!("{ops}_escaping_the_residency_fact"), None),
        ]);
    }
    cells
}

#[test]
fn every_decorator_pair_refuses_every_wrong_shape_by_name() {
    let pairs = declared_pairs();
    baseline::floor(pairs.len(), FLOOR, "DecoratorPair declaration(s)");

    let fixtures = fixtures();
    // Keyed by fixture name so the five provider-hosted pairs share the single
    // `injectable_on_impl` cell instead of reporting one hole five times.
    let mut holes: BTreeMap<String, String> = BTreeMap::new();
    for pair in &pairs {
        for (cell, must_name) in owed(pair) {
            let filled = match fixtures.get(&cell) {
                None => false,
                Some(snapshot) => match &must_name {
                    None => !snapshot.trim().is_empty(),
                    Some(sibling) => snapshot.contains(sibling),
                },
            };
            if !filled {
                holes
                    .entry(cell)
                    .or_insert_with(|| format!("{} / {}", pair.host, pair.operations));
            }
        }
    }
    let holes: BTreeSet<String> = holes
        .into_iter()
        .map(|(cell, pair)| format!("{cell} :: {pair}"))
        .collect();

    baseline::gate(
        BASELINE,
        &holes,
        pairs.iter().map(|p| owed(p).len()).sum::<usize>(),
        "cells",
        "decorator pair × wrong shape",
        "a wrong shape no fixture pins",
    );
}
