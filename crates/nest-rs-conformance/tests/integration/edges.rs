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
//! generic `#[injectable]`, which is the **request-carrying** set the list in
//! `framework.md` binds. It is *not* `architecture.md`'s closed edge vocabulary,
//! which this paragraph claimed and which has seven members: `queue`,
//! `schedule` and `events` are edges and owe none of this list, having no mount,
//! no transport and no posture. `units.rs` cites the same sentence for the
//! seven-member set, correctly — a reader who trusted the claim here would prune
//! that constant to four and silently drop the namespace check for
//! `queue.job`, `schedule.tick` and every `events.*`. The edge is spelled by its
//! crate: `nest-rs-ws-macros`
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
use nest_rs_conformance::sources::{
    carries_a_test, declared_pairs, declared_targets, declares_an_item, idents, is_cfg_test,
    parsed, read, repo_root, rust_files, suite_runs_tests,
};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;

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

    fn calls(&self, ident: &str) -> bool {
        self.calls.contains(ident)
    }
}

/// Whether the edge's runtime crate **declares** `nest_rs::<edge>` as a
/// constant, which is what `framework.md` item 15 asks for: "The target is a
/// constant from `nest_rs_core::target`, never a string."
///
/// Read off the declaration rather than off any string in the file, because
/// `Vocabulary` parses the file as a `TokenStream` and `proc-macro2` lowers
/// `///` and `//!` into `#[doc = "…"]` — a `Literal`. So a doc comment quoting
/// the target filled this cell, and so did a `#[cfg(test)]` assertion naming
/// it: four of the five edges had such a line, and deleting `pub const TARGET`
/// from any of them left the cell green. That is the same shape this join
/// records having corrected in itself, where `"authorize.rs".is_file()` closed
/// a cell for a day — a doc comment says nothing about what is emitted.
fn declares_target(name: &str) -> bool {
    let target = format!("nest_rs::{name}");
    let krate = format!("nest-rs-{name}");
    declared_targets()
        .iter()
        .any(|(value, declaring, _)| *value == target && *declaring == krate)
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

/// The vocabulary a crate's **shipped** code spells.
///
/// `#[cfg(test)]` items are dropped, and that is the correction this join
/// already made at one column and owed at the other twelve: the harvest read
/// every token in a crate's `src`, fixtures included, so a symbol a decorator
/// only ever writes inside its own unit tests filled an edge's cell. It was
/// live — HTTP's per-argument-pipe cell was satisfied **solely** by
/// `parse_quote!(Piped<Trim, …>)` inside `routes.rs`'s `#[cfg(test)] mod`, so
/// deleting the whole pipe-carrier unwrapping left the cell green.
///
/// The same fix at item 15 records the shape ("a doc comment says nothing about
/// what is emitted"); `CLAUDE.md` says why it could not stop there — "a fix
/// scoped to the member that reported it repairs the report, not the defect".
fn vocabulary(dir: &Path) -> Vocabulary {
    let mut out = Vocabulary::default();
    for path in rust_files(dir) {
        let Some(ast) = parsed(&path) else {
            continue;
        };
        for item in &ast.items {
            if is_cfg_test(attrs_of(item)) {
                continue;
            }
            // **An import is not an implementation.** `use nest_rs_codegen::PostureRules;`
            // is a top-level item whose tokens carry the ident, so harvesting it
            // made every ident column an *import*-presence column: appending one
            // dead `use nest_rs_codegen::PostureRules as _Probe;` to
            // `nest-rs-http-macros/src/lib.rs` flipped HTTP's item-2 cell to
            // covered and demanded the baseline line — which carries an argued
            // asymmetry — be deleted, because the baseline only shrinks. Eight
            // of the fourteen columns were defeatable that way.
            //
            // Dropped rather than filtered by root, because there is no import
            // whose presence proves the obligation: what the columns ask is that
            // this edge's own code names the symbol, and a `use` is the one
            // statement that names something precisely because the code does not
            // yet.
            if matches!(item, syn::Item::Use(_)) {
                continue;
            }
            harvest(item.to_token_stream(), &mut out);
        }
    }
    out
}

/// A top-level item's attributes, for the `#[cfg(test)]` question. `syn` gives
/// no uniform accessor, and the shapes a `#[cfg(test)]` legitimately sits on are
/// few: a `mod`, a `fn`, an `impl`, a `use`.
fn attrs_of(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn harvest(tokens: TokenStream, out: &mut Vocabulary) {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    for (i, tree) in trees.iter().enumerate() {
        if let TokenTree::Ident(ident) = tree {
            out.idents.insert(ident.to_string());
            // `f(` or `f::<T>(` — the two shapes a call takes. The turbofish is
            // what a generic gate is written as, and reading only `(` would miss
            // every one of them.
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
    }
    for tree in trees {
        if let TokenTree::Group(group) = tree {
            harvest(group.stream(), out);
        }
    }
}

/// Whether the edge ships the posture refusal `framework.md` item 2 names, as a
/// trybuild snapshot the diagnostics harness runs.
///
/// **Read off the `.stderr` text, never the file name.** This join already
/// recorded a rename closing two cells that asked `"authorize.rs".is_file()`,
/// and a snapshot is exactly the artefact where the *wording* is the contract —
/// each edge's diagnostics module says so in its own doc. Every harness globs
/// `diagnostics/*.rs`, so a file that is there is a file that runs.
///
/// Two sentences, because item 2 names two cases:
///
/// - the three edges with a **mandatory** posture refuse the no-posture
///   declaration with the sentence `PostureRules` words once;
/// - **HTTP's posture is optional** — a route's gate may be `#[use_guards]` —
///   so it has no no-posture case, and the rule names its contradiction
///   instead: `#[authorize]` on an `#[sse]` route, where the mask has no wire
///   model to reconcile against.
fn ships_a_posture_snapshot(root: &Path, edge: &str) -> bool {
    let needle = if edge == "http" {
        "cannot arm a `#[sse]` route"
    } else {
        "declares its access posture"
    };
    let dir = root.join(format!(
        "crates/nest-rs-{edge}/tests/integration/diagnostics"
    ));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.extension().is_some_and(|ext| ext == "stderr")
            && read(&path).is_ok_and(|text| text.contains(needle))
    })
}

/// Whether the edge strips a per-argument pipe carrier — item 7.
///
/// **Two shapes, because `request-layers.md` says there are two by design**:
/// "two forms by design (orphan rule): HTTP wraps an extractor
/// (`nest_rs_http::Piped<P, E>`); GraphQL, WS, MCP and queue wrap the wire
/// value (`nest_rs_pipes::Piped<P, T>`, stripped by the impl-half decorator)".
/// So the proof differs by shape, and asking one question of both is what let a
/// **re-export** answer it: `pub use nest_rs_pipes::{Piped, Valid};` in
/// `nest-rs-mcp/src/lib.rs` spells the ident without the decorator stripping
/// anything, and the cell would have stayed green with `#[tools]`' pipe support
/// deleted. Naming a type is not supporting it.
///
/// - **The three wire-value edges** must reach the shared parser,
///   [`nest_rs_codegen::pipe_wrapper`] — a *call*, so the carrier is actually
///   taken apart, and the shared one, so the grammar cannot fork per edge.
/// - **HTTP** strips nothing: poem's `FromRequest` does the work, which is the
///   orphan-rule half. What it owes is the carrier itself, so the proof is that
///   its runtime crate **declares** `struct Piped` rather than importing one.
fn strips_a_pipe_carrier(edge: &Edge, root: &Path) -> bool {
    if edge.name == "http" {
        return declares_struct(&root.join("crates/nest-rs-http/src"), "Piped");
    }
    edge.macros.calls("pipe_wrapper")
}

/// Whether any shipped file under `dir` declares `struct <name>`.
fn declares_struct(dir: &Path, name: &str) -> bool {
    rust_files(dir).into_iter().any(|path| {
        parsed(&path).is_some_and(|ast| {
            ast.items.iter().any(|item| {
                matches!(item, syn::Item::Struct(s)
                    if s.ident == name && !is_cfg_test(&s.attrs))
            })
        })
    })
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
            // **The other half of item 2, and the half that is the rule's own
            // testable form**: "each of the four edges ships the no-posture
            // (or, for HTTP, the contradiction) trybuild snapshot." The
            // `PostureRules` cell above says *whose grammar*, which two edges
            // argue their way out of; this one says the refusal exists and is
            // executed, which none of them may. Without it item 2 was a single
            // import away from green — a bare `use nest_rs_codegen::PostureRules;`
            // closes an ident column, and `#[authorize]` becoming optional
            // everywhere would not have moved a cell.
            "2. a posture refusal snapshot the harness runs",
            ships_a_posture_snapshot(root, name),
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
            // The **witness**, so the file has to run something: `is_file()`
            // was the whole question for a round, and a `mask.rs` truncated to
            // zero bytes answered it.
            carries_a_test(&root.join(format!(
                "crates/nest-rs-authz/tests/integration/{name}/mask.rs"
            ))) && (edge.macros.calls("masked_value_for") || edge.macros.calls("masked_reply_for")),
        ),
        (
            // **Both halves, because item 5 is two sentences.** "Guards at two
            // scopes — `#[use_guards]` on the host and per operation, plus
            // `#[force_guards]` … A denial renders through one
            // `denial_to_<edge>_error`." The renderer alone was the column for
            // a round, and it is the half that lives *outside* the edge: an
            // edge that bridged no guards at all still had one, because
            // `nest-rs-guards` writes all four. `force_guards` is the scoped
            // half's symbol — the re-run opt-in exists only where the impl-half
            // decorator composes a per-operation chain to re-run into — and
            // all four macros crates bind it as an ident in shipped code.
            "5. two guard scopes and one denial renderer",
            // Deliberately workspace-wide: `denial_to_<edge>_error` lives in
            // `nest-rs-guards`, which is above the transports and cannot be
            // reached from the edge's own two crates.
            framework.contains(&format!("denial_to_{name}_error"))
                && edge.macros.has("force_guards"),
        ),
        ("7. per-argument pipes", strips_a_pipe_carrier(edge, root)),
        (
            "8. a named refusal for every layer family it does not bridge",
            edge.macros.has("reject_http_only_layers"),
        ),
        (
            // Item 9 is also two sentences — "Request scope + a data context —
            // `Scoped<T>`, and an executor+ability re-install per dispatch" —
            // and the second lives in `nest-rs-seaorm`, one module per edge,
            // because the re-install is the ORM's ambient state and not the
            // transport's. Asking for `dispatch::with_data_context` by name
            // would have opened a false hole at GraphQL, which re-installs in
            // the dataloader spawner instead: `/graphql` is an HTTP self-mount,
            // so the request's own context came from HTTP's interceptor and
            // what needs re-installing is the part that outlives it. The module
            // is what all four have.
            "9. request scope and a data context",
            edge.has("Scoped")
                && declares_an_item(&root.join(format!("crates/nest-rs-seaorm/src/{name}"))),
        ),
        ("11. error opacity", edge.runtime.has("Opaque")),
        // **The `Discoverable` half only, and the gate half deliberately not.**
        // Item 12 offers a *choice* — "`ReachableProviders` for a link-time
        // registry **or** structural gating for container metadata" — and
        // `framework.md` says of the second "there is nothing to filter and no
        // inert-entry `warn` to emit". So a gate column would be an
        // either-or over two symbols, one of which (`HttpEndpointMeta`) is
        // already item 14's, and it would read as a check while asserting the
        // disjunction of a thing and its own alternative. Three of the four
        // edges gate structurally; naming that here is the finding, not a cell.
        ("12. discovery", edge.has("Discoverable")),
        (
            // The rule's "or an HTTP self-mount **declaring its
            // `EdgePosture`**" needs no second symbol: `HttpEndpointMeta::new`
            // sets `EdgePosture::Guarded` and `exempt()` is the only way off
            // it, so a meta that exists has declared one. Checked rather than
            // assumed — a posture that ever becomes optional makes this column
            // a hole, and the sentence is here for whoever changes it.
            "14. a mount",
            edge.has("TransportContribution") || edge.has("HttpEndpointMeta"),
        ),
        ("15. the nest_rs::<edge> span target", declares_target(name)),
        (
            "16. an integration suite",
            // A suite that runs nothing is not a suite. `main.rs.is_file()` was
            // the cell, and truncating that file to zero bytes left it green —
            // which is the shape this join records correcting twice already.
            suite_runs_tests(
                &root.join(format!("crates/nest-rs-{name}/tests/integration/main.rs")),
            ),
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
        .any(|entry| declares_an_item(&entry.path().join(edge)))
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
