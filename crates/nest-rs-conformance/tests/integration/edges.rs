//! The edges join: the four request-carrying edges, against the list
//! `framework.md` says each of them owes.
//!
//! *A new edge owes the same list — and the list is here* is sixteen numbered
//! items, each carrying the grep or the test that proves it, under the sentence
//! "**A line whose proof you cannot run is a line you have not done.**" Nobody
//! ran them. This runs the ones that are a symbol or a file; the rest are named
//! below as not mechanisable, which is a different sentence from *not yet done*
//! and is written as one.
//!
//! Members are the four edge pairs — a `DecoratorPair` whose host is **not** the
//! generic `#[injectable]`, which is exactly the set `architecture.md` calls the
//! closed edge vocabulary. The edge is spelled by its crate: `nest-rs-ws-macros`
//! is the `ws` edge, and `nest-rs-ws` plus `nest-rs-ws-macros` are read together
//! because an edge *is* those two crates — the host decorator and the runtime it
//! expands into.
//!
//! **Three items are delegated, not skipped**, because another join already owns
//! them and a second test for an occupied cell is what clause 2 forbids:
//!
//! - item 1 (the pair and its wrong-shape snapshots) is the `shapes` join;
//! - item 10 (`#[config]` + `for_root`) is the `seams` join;
//! - item 6's marker-and-bound half is the `guards` join.
//!
//! **Two items are reported as not mechanisable**, and neither is a stub:
//!
//! - **item 13, aggregation.** What it asks is that a duplicate addressable name
//!   fails the *boot* naming both owners. There is no symbol that says a boot
//!   error exists and is reachable, and WS is a recorded exception that owns its
//!   mount outright — so a column here would either pass on the presence of an
//!   error type nobody raises, or fail on the one edge the rule excuses. It is a
//!   behavioural assertion, which is what the edge crates' own suites are for.
//! - **item 16's testing driver**, which the rule makes conditional — "a driver
//!   in `nest-rs-testing` **if the protocol needs one**". Nothing derives whether
//!   a protocol needs one, so a column would be asserting a judgement, not a
//!   fact.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{declared_pairs, idents, read, repo_root, rust_files};
use proc_macro2::{Delimiter, TokenStream, TokenTree};

const BASELINE: &str = "edges-baseline.txt";

/// Four edges stand today, and `architecture.md` calls the vocabulary closed.
/// Below that the scan is reading the wrong tree.
const FLOOR: usize = 4;

/// One edge: its name, and everything its two crates spell.
struct Edge {
    /// `http`, `ws`, `graphql`, `mcp` — the `<edge>` every item interpolates.
    name: String,
    /// Identifiers and string literals from `nest-rs-<edge>/src`.
    runtime: Vocabulary,
    /// The same, from `nest-rs-<edge>-macros/src`.
    macros: Vocabulary,
}

#[derive(Default)]
struct Vocabulary {
    idents: BTreeSet<String>,
    literals: BTreeSet<String>,
    /// Idents that are *invoked* — followed by `(` or by a turbofish. An ident
    /// alone cannot tell a decision from a module path: `gate::warn_denied`
    /// spells `gate` while calling nothing, and reading that as "this edge's
    /// gate decides through the shared one" is the loose match the rule warns
    /// about, in the direction that costs a wrong answer.
    calls: BTreeSet<String>,
}

impl Vocabulary {
    fn has(&self, ident: &str) -> bool {
        self.idents.contains(ident)
    }

    fn quotes(&self, literal: &str) -> bool {
        self.literals.iter().any(|lit| lit.contains(literal))
    }

    fn calls(&self, ident: &str) -> bool {
        self.calls.contains(ident)
    }
}

impl Edge {
    /// Either crate — an edge is the pair, and which half holds a symbol is an
    /// implementation detail the rule does not fix.
    fn has(&self, ident: &str) -> bool {
        self.runtime.has(ident) || self.macros.has(ident)
    }

    fn cell(&self, item: &str) -> String {
        format!("{} :: {item}", self.name)
    }
}

fn vocabulary(dir: &Path) -> Vocabulary {
    let mut out = Vocabulary::default();
    for path in rust_files(dir) {
        let Ok(text) = read(&path) else {
            continue;
        };
        let Ok(tokens) = text.parse::<TokenStream>() else {
            continue;
        };
        harvest(tokens, &mut out);
    }
    out
}

fn harvest(tokens: TokenStream, out: &mut Vocabulary) {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    for (i, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Ident(ident) => {
                out.idents.insert(ident.to_string());
                // `f(` or `f::<T>(` — the two shapes a call takes. The
                // turbofish is what a generic gate is written as, and reading
                // only `(` would miss every one of them.
                let invoked = match trees.get(i + 1) {
                    Some(TokenTree::Group(g)) => g.delimiter() == Delimiter::Parenthesis,
                    Some(TokenTree::Punct(p)) if p.as_char() == ':' => matches!(
                        trees.get(i + 2..i + 4),
                        Some([TokenTree::Punct(_), TokenTree::Punct(lt)]) if lt.as_char() == '<'
                    ),
                    _ => false,
                };
                if invoked {
                    out.calls.insert(ident.to_string());
                }
            }
            TokenTree::Literal(lit) => {
                out.literals.insert(lit.to_string());
            }
            _ => {}
        }
    }
    for tree in trees {
        if let TokenTree::Group(group) = tree {
            harvest(group.stream(), out);
        }
    }
}

/// The four edges, from the decorator pairs that declare them.
fn edges(root: &Path) -> Vec<Edge> {
    declared_pairs()
        .into_iter()
        // A provider-hosted pair (`#[processor]`, `#[scheduled]`, …) has no
        // mount, no transport and no posture: it is not an edge, and the list
        // this join runs does not bind it.
        .filter(|pair| pair.host != "#[injectable]")
        .filter_map(|pair| {
            let name = pair
                .krate
                .strip_prefix("nest-rs-")?
                .strip_suffix("-macros")?
                .to_owned();
            Some(Edge {
                runtime: vocabulary(&root.join("crates").join(format!("nest-rs-{name}/src"))),
                macros: vocabulary(
                    &root
                        .join("crates")
                        .join(format!("nest-rs-{name}-macros/src")),
                ),
                name,
            })
        })
        .collect()
}

/// The edge's own half of `nest-rs-authz`, as one vocabulary — the source that
/// decides and masks for it.
fn authz_edge(root: &Path, edge: &str) -> Vocabulary {
    vocabulary(&root.join(format!("crates/nest-rs-authz/src/{edge}")))
}

/// What every edge owes, item by item, in `framework.md`'s own numbering.
///
/// Each closure is the parenthesised proof the rule already wrote down; nothing
/// here invents an obligation, and an item whose proof is a sentence rather than
/// a symbol is in the module doc instead of this list.
fn owed(edge: &Edge, root: &Path, framework: &BTreeSet<String>) -> Vec<(&'static str, bool)> {
    let name = &edge.name;
    vec![
        (
            "2. posture parsed by the shared PostureRules",
            edge.macros.has("PostureRules"),
        ),
        (
            // The rule's own words: "whose *decision* is the shared `gate` so
            // `#[authorize]` cannot come to mean five things". So the proof is
            // the call, not the file. It read `authorize.rs".is_file()` for a
            // day, and a rename closed a cell that way — a filename says
            // nothing about what is in it, and `testing.md` clause 3 asks for a
            // proved cell.
            "3. a class gate whose decision is the shared gate",
            authz_edge(root, name).calls("gate"),
        ),
        (
            // Likewise: the rule names the file *and* what it asserts. The
            // masking entry point is the edge's own — `masked_value_for` where
            // the return type must be reconstructed, `masked_reply_for` where
            // the wire is JSON — and the witness has to reach one of them.
            "4. a mask witness reaching the edge's masking entry point",
            root.join(format!(
                "crates/nest-rs-authz/tests/integration/{name}/mask.rs"
            ))
            .is_file()
                && (edge.macros.calls("masked_value_for") || edge.macros.calls("masked_reply_for")),
        ),
        (
            "5. one denial renderer",
            // Deliberately workspace-wide: `denial_to_<edge>_error` lives in
            // `nest-rs-guards`, which is above the transports and cannot be
            // reached from the edge's own two crates.
            framework.contains(&format!("denial_to_{name}_error")),
        ),
        ("7. per-argument pipes", edge.macros.has("Piped")),
        (
            "8. a named refusal for every layer family it does not bridge",
            edge.macros.has("reject_http_only_layers"),
        ),
        ("9. request scope", edge.has("Scoped")),
        ("11. error opacity", edge.runtime.has("Opaque")),
        ("12. discovery", edge.has("Discoverable")),
        (
            "14. a mount",
            edge.has("TransportContribution") || edge.has("HttpEndpointMeta"),
        ),
        (
            "15. the nest_rs::<edge> span target",
            edge.runtime.quotes(&format!("nest_rs::{name}")),
        ),
        (
            "16. an integration suite",
            root.join(format!("crates/nest-rs-{name}/tests/integration/main.rs"))
                .is_file(),
        ),
        ("16. an adapter in demo/", has_demo_adapter(root, name)),
    ]
}

/// Every identifier any framework crate's `src` spells.
///
/// Read once for the whole join rather than per edge: the one workspace-wide
/// column asks four questions, and answering each by re-walking 800 files was
/// four passes over the same 5 MB.
fn framework_idents(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for path in rust_files(&root.join("crates")) {
        let Ok(text) = read(&path) else {
            continue;
        };
        let Ok(tokens) = text.parse::<TokenStream>() else {
            continue;
        };
        out.extend(idents(tokens));
    }
    out
}

/// `demo/crates/features/src/<feature>/<edge>/` — the adapter layout
/// `features.md` mandates, so the edge is spelled by a directory.
fn has_demo_adapter(root: &Path, edge: &str) -> bool {
    let features = root.join("demo/crates/features/src");
    let Ok(entries) = std::fs::read_dir(&features) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().join(edge).is_dir())
}

#[test]
fn every_edge_owes_the_same_list() {
    let root = repo_root();
    let edges = edges(&root);
    baseline::floor(edges.len(), FLOOR, "edge(s)");

    let framework = framework_idents(&root);
    let mut holes = BTreeSet::new();
    let mut cells = 0usize;
    for edge in &edges {
        for (item, present) in owed(edge, &root, &framework) {
            cells += 1;
            if !present {
                holes.insert(edge.cell(item));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        cells,
        "cells",
        "edge × obligation",
        "an item the edge owes and the source does not carry",
    );
}
