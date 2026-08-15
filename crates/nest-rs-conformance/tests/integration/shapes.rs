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
use nest_rs_conformance::sources::{parsed, repo_root, rust_files};
use syn::{Expr, Item, Lit};

const BASELINE: &str = "shapes-baseline.txt";

/// Nine pairs stand today: four edges plus five whose struct half is the
/// generic `#[injectable]`. Below that the scan is reading the wrong tree.
const FLOOR: usize = 9;

/// A pair as its own declaration spells it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Pair {
    /// The struct half — `#[injectable]` for a provider-hosted pair.
    host: String,
    /// The impl half.
    operations: String,
}

/// `#[controller]` → `controller`, which is how the fixtures spell it.
fn bare(decorator: &str) -> String {
    decorator
        .trim_start_matches("#[")
        .trim_end_matches(']')
        .to_owned()
}

fn as_str_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

/// Both spellings of a declaration, and only these two: a third would be a
/// second way to declare a pair, which the rule forbids before this join has
/// to care.
fn pair_from(expr: &Expr) -> Option<(String, String)> {
    match expr {
        // `DecoratorPair { host: "#[controller]", operations: "#[routes]", .. }`
        Expr::Struct(lit) if lit.path.segments.last()?.ident == "DecoratorPair" => {
            let field = |name: &str| {
                lit.fields
                    .iter()
                    .find(|f| matches!(&f.member, syn::Member::Named(i) if i == name))
                    .and_then(|f| as_str_lit(&f.expr))
            };
            Some((field("host")?, field("operations")?))
        }
        // `DecoratorPair::on_provider("#[processor]", "#[process]")` — the host
        // is the generic `#[injectable]`, which is why five pairs share one
        // host cell rather than owing five.
        Expr::Call(call) => {
            let Expr::Path(path) = &*call.func else {
                return None;
            };
            if path.path.segments.last()?.ident != "on_provider" {
                return None;
            }
            let operations = as_str_lit(call.args.first()?)?;
            Some(("#[injectable]".to_owned(), operations))
        }
        _ => None,
    }
}

fn declared_pairs() -> Vec<Pair> {
    let root = repo_root();
    let mut out = Vec::new();
    for dir in std::fs::read_dir(root.join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let path = dir.path();
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-macros"))
        {
            continue;
        }
        for file in rust_files(&path.join("src")) {
            let Some(ast) = parsed(&file) else {
                continue;
            };
            for item in &ast.items {
                let Item::Const(konst) = item else {
                    continue;
                };
                if let Some((host, operations)) = pair_from(&konst.expr) {
                    out.push(Pair { host, operations });
                }
            }
        }
    }
    out
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
    assert!(
        pairs.len() >= FLOOR,
        "the scan found {} DecoratorPair declaration(s) — below {FLOOR} it is \
         reading the wrong tree, and every hole it reports is an artefact",
        pairs.len(),
    );

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

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/integration")
        .join(BASELINE);
    let Some(verdict) = baseline::compare(&path, &holes) else {
        baseline::land(&path, &holes);
        panic!(
            "no baseline: wrote {} of {} cells as today's holes. Read it before \
             committing — every line is a wrong shape no fixture pins.",
            holes.len(),
            pairs.iter().map(|p| owed(p).len()).sum::<usize>(),
        );
    };
    assert!(
        verdict.is_clean(),
        "{}",
        verdict.report("decorator pair × wrong shape"),
    );
}
