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

/// Every trybuild fixture in the workspace, by bare file name. The population is
/// workspace-wide because a pair's refusal is proved in its *surface* crate,
/// never in the macro crate that emits it.
fn fixtures() -> BTreeSet<String> {
    let root = repo_root();
    rust_files(&root.join("crates"))
        .into_iter()
        .filter(|p| p.to_string_lossy().contains("/diagnostics/"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .collect()
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
fn owed(pair: &Pair) -> Vec<String> {
    let host = bare(&pair.host);
    let ops = bare(&pair.operations);
    let mut cells = vec![
        format!("{host}_on_impl"),
        format!("{ops}_on_struct"),
        format!("{ops}_on_a_trait_impl"),
        format!("{ops}_takes_no_arguments"),
    ];
    if pair.host == "#[injectable]" {
        cells.extend([
            format!("{ops}_on_a_non_provider"),
            format!("{ops}_on_a_request_scoped_provider"),
            format!("{ops}_on_a_transient_provider"),
            format!("{ops}_escaping_the_residency_fact"),
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
        for cell in owed(pair) {
            if !fixtures.contains(&cell) {
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
