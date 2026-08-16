//! The guards join: every site that binds a guard capability marker, against
//! what `framework.md` item 6 makes that marker owe.
//!
//! The bound is the whole mechanism. Every `check_*` on `Guard` defaults to
//! `Ok(())`, so an empty `impl Guard for X {}` compiles and passes everything —
//! the marker is what turns that into an error at the *binding* site, and the
//! rule fixes four things it needs to be real: the marker trait, its
//! `#[diagnostic::on_unimplemented]` note, the `check_<edge>` it attests, and "a
//! trybuild snapshot per edge, binding a guard that does not check it at that
//! edge's site".
//!
//! Members are derived from the emissions themselves: every
//! `guard_capability_bounds(…, ::nest_rs_guards::<Marker>)` in a `*-macros`
//! crate is one binding site, and it is keyed by the **surface crate that emits
//! it** paired with the marker. That pairing is what makes the join sharper than
//! a per-marker one: `nest-rs-ws` emits *two* markers — `WsGuard` per message,
//! and `HttpGuard` from the `#[gateway]` struct, whose guards run on the upgrade
//! — so a per-marker view would see `HttpGuard` covered by HTTP's own snapshots
//! and never ask whether the gateway site has one. It does not, and the rule
//! already says so: "two carry a snapshot today and the gateway site does not".
//!
//! **The global site is a reported residue, not a member.** `use_guards_global`
//! (`nest-rs-guards/src/builder.rs`) takes no capability bound at all, so it
//! emits no `guard_capability_bounds` call and this population never reaches it.
//! That is the *correct* reading rather than a miss: the population is sites
//! that bind a capability, and the residue is precisely that one site binds
//! none. `framework.md` records why bounding it is not the fix — a global guard
//! legitimately serves whichever edges it implements, and requiring `HttpGuard`
//! would refuse a GraphQL-only one. A column asserting the bound exists there
//! would fail forever on a decision already taken.
//!
//! **The phase-validation column is wider than the residue `framework.md`
//! records**, and deliberately. The rule names `#[tools]` and `#[operations]` as
//! the two chains that "compose their chain at runtime and emit no such check".
//! The column asks the mechanical question — does this site's own source call
//! `boot_validate_guards` — and the answer puts WS's two sites in the same
//! position. That is a fact about the emissions, not a claim that WS is
//! fail-open; the rule's argument for why this is a diagnostic gap rather than a
//! hole ("it fails **closed** — the ability guard finds no principal and
//! installs nothing, so `Repo` denies") is untouched by it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    files_with_extension, flatten, parsed, read, repo_root, rust_files,
};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::Item;

const BASELINE: &str = "guards-baseline.txt";

/// Five binding sites stand today — HTTP's two decorators collapse to one
/// `(crate, marker)` cell, as do MCP's. Below this the scan is reading the wrong
/// tree.
const FLOOR: usize = 5;

/// One place a decorator emits a capability bound.
#[derive(Debug, Clone)]
struct Site {
    /// The surface crate whose suite owes the snapshot — `nest-rs-ws`.
    krate: String,
    /// The marker it emits — `HttpGuard`.
    marker: String,
    /// Whether a decorator of this crate phase-validates **the chain this
    /// marker guards** — not merely some chain of the crate's.
    ///
    /// Keyed by marker through [`attested_marker`], because a crate can validate
    /// one of its chains and not the other, and that is the live case:
    /// `#[messages]` validates a gateway's *upgrade* chain (`HttpGuard`) and
    /// deliberately not its per-message ones (`WsGuard`). A per-crate fold read
    /// the second as covered off the strength of the first — the exact silence
    /// this column exists to break.
    ///
    /// Folded across the crate's **files**, though: a decorator pair is two item
    /// shapes in two files and the check belongs to whichever half emits
    /// `Discoverable::register`, so `#[gateway]` declares the upgrade guards
    /// while `#[messages]` validates them.
    validates: bool,
}

impl Site {
    /// `HttpGuard` → `check_http…`, the entry the marker attests.
    ///
    /// A prefix rather than the whole name, because WS's entry is
    /// `check_ws_message`: the marker attests *per message* rather than at the
    /// connection, which `framework.md` states outright ("`WsGuard`, per message
    /// rather than at the upgrade"). Demanding `check_ws` would report a hole
    /// where the asymmetry is the design.
    fn check_prefix(&self) -> String {
        format!(
            "check_{}",
            self.marker.trim_end_matches("Guard").to_lowercase()
        )
    }

    fn cell(&self, column: &str) -> String {
        format!("{} emits {} :: {column}", self.krate, self.marker)
    }
}

/// Every `guard_capability_bounds(…, ::nest_rs_guards::<Marker>)` call.
///
/// Read as tokens because the marker travels inside `quote!(…)` — a macro
/// invocation whose body `syn` keeps as an opaque `TokenStream`, so an
/// expression visitor sees the call and not the argument that identifies it.
fn binding_sites(root: &Path) -> Vec<Site> {
    let mut markers: BTreeSet<(String, String)> = BTreeSet::new();
    /// Which marker's chain a `boot_validate_*` in `nest-rs-guards` attests.
    ///
    /// Named rather than prefix-matched: the point of the column is *which*
    /// chain got checked, and a prefix cannot say. A new entry here is what a
    /// new validator owes — and forgetting it reads as a hole, which is the
    /// direction an unproved column should fail in.
    fn attested_marker(call: &str) -> Option<&'static str> {
        match call {
            // A request-scope chain: a controller's routes, or a gateway's
            // upgrade — both run `check_http`.
            "boot_validate_guards" => Some("HttpGuard"),
            _ => None,
        }
    }
    let mut validated: BTreeSet<(String, String)> = BTreeSet::new();
    for dir in std::fs::read_dir(root.join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let path = dir.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(surface) = name.strip_suffix("-macros") else {
            continue;
        };
        for file in rust_files(&path.join("src")) {
            let Ok(text) = read(&file) else {
                continue;
            };
            let Ok(tokens) = text.parse::<TokenStream>() else {
                continue;
            };
            let mut flat = Vec::new();
            flatten(tokens.clone(), &mut flat);
            // A **call**, not a mention: the ident has to be followed by its
            // argument list. Matching the bare ident anywhere in the crate's
            // `src` let a `use ::nest_rs_guards::dispatch::boot_validate_guards;`
            // — an import that runs nothing — flip a cell green, and with it
            // delete the baseline line carrying an owner question.
            for pair in flat.windows(2) {
                let [TokenTree::Ident(called), TokenTree::Group(args)] = pair else {
                    continue;
                };
                if args.delimiter() != Delimiter::Parenthesis {
                    continue;
                }
                if let Some(marker) = attested_marker(&called.to_string()) {
                    validated.insert((surface.to_owned(), marker.to_owned()));
                }
            }
            for marker in markers_bound_in(tokens.clone()) {
                markers.insert((surface.to_owned(), marker));
            }
        }
    }
    markers
        .into_iter()
        .map(|key| Site {
            validates: validated.contains(&key),
            krate: key.0,
            marker: key.1,
        })
        .collect()
}

/// The markers named inside a `guard_capability_bounds(…)` call's own argument
/// list — never one merely imported at the top of the file.
fn markers_bound_in(tokens: TokenStream) -> Vec<String> {
    let mut flat = Vec::new();
    flatten(tokens, &mut flat);
    let mut out = Vec::new();
    for (index, tree) in flat.iter().enumerate() {
        let TokenTree::Ident(ident) = tree else {
            continue;
        };
        if ident != "guard_capability_bounds" {
            continue;
        }
        // The call's parentheses are the next group; the marker is the only
        // `*Guard` identifier inside it, at whatever `quote!` nesting depth.
        let Some(TokenTree::Group(group)) = flat.get(index + 1) else {
            continue;
        };
        let mut inner = Vec::new();
        flatten(group.stream(), &mut inner);
        out.extend(inner.iter().filter_map(|tree| match tree {
            TokenTree::Ident(ident) => {
                let name = ident.to_string();
                (name.ends_with("Guard") && name != "Guard").then_some(name)
            }
            _ => None,
        }));
    }
    out.sort();
    out.dedup();
    out
}

/// What `nest-rs-guards`' own traits declare: every capability marker paired
/// with whether it carries the `#[diagnostic::on_unimplemented]` note the rule
/// requires, and every `check_*` entry on `Guard`.
///
/// One walk for both, because they read the same `Item::Trait` items of the same
/// tree and the join needs them together.
fn guard_surface(root: &Path) -> (BTreeMap<String, bool>, BTreeSet<String>) {
    let mut markers = BTreeMap::new();
    let mut entries = BTreeSet::new();
    for file in rust_files(&root.join("crates/nest-rs-guards/src")) {
        let Some(ast) = parsed(&file) else {
            continue;
        };
        for item in &ast.items {
            let Item::Trait(t) = item else {
                continue;
            };
            if t.ident == "Guard" {
                entries.extend(t.items.iter().filter_map(|member| match member {
                    syn::TraitItem::Fn(f) => Some(f.sig.ident.to_string()),
                    _ => None,
                }));
                continue;
            }
            let name = t.ident.to_string();
            if !name.ends_with("Guard") {
                continue;
            }
            // `<Edge>Guard: Guard` — a supertrait bound on `Guard` is what makes
            // it a capability marker rather than some other trait ending in the
            // same word.
            let attests = t.supertraits.iter().any(|bound| {
                matches!(bound, syn::TypeParamBound::Trait(t)
                    if t.path.segments.last().is_some_and(|s| s.ident == "Guard"))
            });
            if !attests {
                continue;
            }
            let noted = t.attrs.iter().any(|attr| {
                attr.path()
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "on_unimplemented")
            });
            markers.insert(name, noted);
        }
    }
    (markers, entries)
}

/// Which markers each crate's trybuild snapshots refuse a guard for.
///
/// The `.stderr` is the whole point: a marker named in a passing test says a
/// guard *has* the capability, while a marker named in a snapshot is the
/// compiler refusing one that has not. Only the second is the witness.
///
/// The markers looked for are `declared_markers`' own keys, never a literal
/// list: a fifth edge declares a marker, becomes a `Site`, and a hardcoded list
/// would report its snapshot as missing however many it ships — a false hole,
/// which under the "baseline only shrinks" discipline is closed by writing an
/// excuse into the baseline rather than by fixing the scan.
fn snapshotted_markers(
    root: &Path,
    markers: &BTreeMap<String, bool>,
) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        return out;
    };
    for entry in entries.flatten() {
        let krate = entry.path();
        let Some(name) = krate.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for path in files_with_extension(&krate.join("tests"), "stderr") {
            let Ok(text) = read(&path) else {
                continue;
            };
            for marker in markers.keys() {
                if text.contains(marker) {
                    out.insert((name.to_owned(), marker.clone()));
                }
            }
        }
    }
    out
}

#[test]
fn every_guard_capability_site_carries_its_marker_and_its_snapshot() {
    let root = repo_root();
    let sites = binding_sites(&root);
    baseline::floor(sites.len(), FLOOR, "guard capability binding site(s)");

    let (markers, entries) = guard_surface(&root);
    let snapshots = snapshotted_markers(&root, &markers);

    let mut holes = BTreeSet::new();
    let mut cells = 0usize;
    for site in &sites {
        let declared = markers.get(&site.marker);
        for (column, present) in [
            (
                "the marker trait, declared as `<Edge>Guard: Guard`",
                declared.is_some(),
            ),
            (
                "a #[diagnostic::on_unimplemented] note on the marker",
                declared.copied().unwrap_or(false),
            ),
            (
                "the Guard::check_<edge> it attests",
                entries
                    .iter()
                    .any(|entry| entry.starts_with(&site.check_prefix())),
            ),
            (
                "a trybuild snapshot refusing an unattested guard at this site",
                snapshots.contains(&(site.krate.clone(), site.marker.clone())),
            ),
            (
                "a boot-time phase validation of the chain it composes",
                site.validates,
            ),
        ] {
            cells += 1;
            if !present {
                holes.insert(site.cell(column));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        cells,
        "cells",
        "guard capability site × obligation",
        "a capability bound the site emits and nothing proves",
    );
}
