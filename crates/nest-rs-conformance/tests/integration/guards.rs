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
//! crate is one binding site, and it is keyed by the **decorator whose
//! expansion emits it** paired with the marker. The decorator, because that is
//! the unit `framework.md` names: "HTTP has **three** emitters —
//! `#[controller]`, `#[routes]` and the `#[gateway]` struct, whose guards run on
//! the upgrade — and each underlines the decorator the guard was written under,
//! **with a snapshot of its own** (`unattested_guard_on_a_gateway` is the
//! third)."
//!
//! **It was keyed by the crate, and that folded three pairs into three cells.**
//! `#[controller]`/`#[routes]`, `#[resolver]`/`#[operations]` and
//! `#[mcp]`/`#[tools]` each emit one marker from two decorators, so one snapshot
//! closed a cell two sites owed and deleting either left it green — the
//! defeatable shape `testing.md` clause 3 forbids. Under the decorator key two
//! of those snapshots turned out never to have been written
//! (`http_only_guard_on_an_operation`, `http_only_guard_on_a_tool_operation`),
//! which is what the collapse had been hiding.
//!
//! Resolving decorator → emission is a **function**-level walk from `lib.rs`,
//! not a module-level one: `nest-rs-mcp-macros/src/mcp.rs` holds the entry of
//! both MCP decorators and only `tools` delegates on into `mcp_impl`, so a
//! module closure would hand `#[mcp]` every marker `#[tools]` binds and put the
//! two straight back in one cell.
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
use quote::ToTokens;
use syn::Item;

const BASELINE: &str = "guards-baseline.txt";

/// Eight binding sites stand today — two decorators per edge. Below this the
/// scan is reading the wrong tree.
///
/// **It was five, and the missing three were the second emitter of HTTP,
/// GraphQL and MCP.** Keying a site by `(crate, marker)` folded `#[controller]`
/// into `#[routes]`, `#[resolver]` into `#[operations]` and `#[mcp]` into
/// `#[tools]`, so one snapshot closed a cell two decorators owed and deleting
/// either left it green — while `framework.md` says the opposite outright:
/// "HTTP has **three** emitters … and each underlines the decorator the guard
/// was written under, **with a snapshot of its own**." Two of those snapshots
/// did not exist and were written when this key exposed them.
const FLOOR: usize = 8;

/// One place a decorator emits a capability bound.
#[derive(Debug, Clone)]
struct Site {
    /// The surface crate whose suite owes the snapshot — `nest-rs-ws`.
    krate: String,
    /// The decorator whose expansion emits the bound — `gateway`, `messages`,
    /// `tools`. **This is what makes a site a site**: a crate can emit one
    /// marker from two decorators, and `framework.md` gives each its own
    /// snapshot, "underlin[ing] the decorator the guard was written under".
    decorator: String,
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
        format!(
            "{} #[{}] emits {} :: {column}",
            self.krate, self.decorator, self.marker
        )
    }
}

/// One function of a `*-macros` crate: what it emits, and where it delegates.
#[derive(Default)]
struct Function {
    /// Capability markers this function's own body binds.
    markers: Vec<String>,
    /// `(module, function)` it calls — the delegation edges.
    calls: BTreeSet<(String, String)>,
}

/// Every free function of `module`, keyed `(module, name)`.
///
/// **A function-level graph rather than a module-level one**, and that is the
/// whole point of the rewrite: `mcp.rs` holds the entry of *both* MCP
/// decorators — `mcp::mcp` and `mcp::tools` — and only the second reaches
/// `mcp_impl`. A module-level closure would hand `#[mcp]` every marker
/// `#[tools]` binds and put the two back in one cell, which is the collapse
/// this join is correcting.
fn collect_functions(
    module: &str,
    ast: &syn::File,
    graph: &mut BTreeMap<(String, String), Function>,
) {
    for item in &ast.items {
        let Item::Fn(f) = item else {
            continue;
        };
        let tokens = f.block.to_token_stream();
        let mut flat = Vec::new();
        flatten(tokens.clone(), &mut flat);
        let mut calls = BTreeSet::new();
        for window in flat.windows(4) {
            // `a :: b` — the two `Punct`s are how `::` reaches a token stream.
            let [
                TokenTree::Ident(a),
                TokenTree::Punct(first),
                TokenTree::Punct(second),
                TokenTree::Ident(b),
            ] = window
            else {
                continue;
            };
            if first.as_char() == ':' && second.as_char() == ':' {
                calls.insert((a.to_string(), b.to_string()));
            }
        }
        // A bare call inside the same module — `mcp_struct(args, item)`.
        for window in flat.windows(2) {
            let [TokenTree::Ident(called), TokenTree::Group(args)] = window else {
                continue;
            };
            if args.delimiter() == Delimiter::Parenthesis {
                calls.insert((module.to_owned(), called.to_string()));
            }
        }
        graph.insert(
            (module.to_owned(), f.sig.ident.to_string()),
            Function {
                markers: markers_bound_in(tokens),
                calls,
            },
        );
    }
}

/// Which markers each `#[proc_macro_attribute]` reaches, following delegations.
///
/// Roots are `lib.rs`'s attribute macros — Rust forces them to the crate root,
/// which is why the root *is* the decorator list (`framework.md`'s one licensed
/// exception to "`lib.rs` carries no logic").
fn decorator_markers(
    graph: &BTreeMap<(String, String), Function>,
    modules: &BTreeSet<String>,
) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for (key, _) in graph.iter().filter(|((module, _), _)| module == "lib") {
        let decorator = key.1.clone();
        let mut seen = BTreeSet::new();
        let mut stack = vec![key.clone()];
        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            let Some(function) = graph.get(&node) else {
                continue;
            };
            for marker in &function.markers {
                out.insert((decorator.clone(), marker.clone()));
            }
            for (module, name) in &function.calls {
                if modules.contains(module) {
                    stack.push((module.clone(), name.clone()));
                }
            }
        }
    }
    out
}

/// Every `guard_capability_bounds(…, ::nest_rs_guards::<Marker>)` call.
///
/// Read as tokens because the marker travels inside `quote!(…)` — a macro
/// invocation whose body `syn` keeps as an opaque `TokenStream`, so an
/// expression visitor sees the call and not the argument that identifies it.
fn binding_sites(root: &Path) -> Vec<Site> {
    let mut markers: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut graph: BTreeMap<(String, String), Function> = BTreeMap::new();
    let mut modules: BTreeSet<String> = BTreeSet::new();
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
            let Some(module) = file.file_stem().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(ast) = syn::parse_file(&text) else {
                continue;
            };
            modules.insert(module.to_owned());
            collect_functions(module, &ast, &mut graph);
        }
        for (decorator, marker) in decorator_markers(&graph, &modules) {
            markers.insert((surface.to_owned(), decorator, marker));
        }
        graph.clear();
        modules.clear();
    }
    markers
        .into_iter()
        .map(|(krate, decorator, marker)| Site {
            // **Folded across the crate's files on purpose**, unlike the
            // snapshot: a decorator pair is two item shapes in two files and
            // the check belongs to whichever half holds the container, so
            // `#[gateway]` declares the upgrade guards and `#[messages]`
            // validates them.
            validates: validated.contains(&(krate.clone(), marker.clone())),
            krate,
            decorator,
            marker,
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

/// Every trybuild snapshot that **refuses** a guard, as
/// `(crate, decorator underlined, marker refused)`.
///
/// Two discriminators, and the second is the one that was missing. The
/// `.stderr` is the whole point of the first: a marker named in a passing test
/// says a guard *has* the capability, while a marker named in a snapshot is the
/// compiler refusing one that has not. The second is the decorator rustc names
/// under `required by a bound in `__nestrs_assert_guard_capability`` — which is
/// exactly `framework.md`'s "each underlines the decorator the guard was
/// written under". Without it a crate's two emitters shared one cell and either
/// snapshot closed both.
///
/// The markers looked for are `guard_surface`'s own keys, never a literal list:
/// a fifth edge declares a marker, becomes a `Site`, and a hardcoded list would
/// report its snapshot as missing however many it ships — a false hole, which
/// under the "baseline only shrinks" discipline is closed by writing an excuse
/// into the baseline rather than by fixing the scan.
fn snapshotted_sites(root: &Path, markers: &BTreeMap<String, bool>) -> BTreeSet<Snapshot> {
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
            // The **refused** marker, not every marker the sentence lists.
            // A capability refusal ends with a `= note:` offering the edges the
            // guard *does* check — HTTP's `unattested_guard_on_a_controller`
            // names all three siblings there — so a `contains` over the whole
            // snapshot credited `nest-rs-http` with a `WsGuard` cell off a line
            // whose whole purpose is to say the guard belongs somewhere else.
            // The refusal is the `error:` and the `help: the trait … is not
            // implemented` that carries the bound; the remedy is the rest.
            let refused: String = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("= note:"))
                .collect::<Vec<_>>()
                .join("\n");
            for decorator in underlined_decorators(&text) {
                for marker in markers.keys() {
                    if refused.contains(marker) {
                        out.insert((name.to_owned(), decorator.clone(), marker.clone()));
                    }
                }
            }
        }
    }
    out
}

/// `(crate, decorator, marker)` — one snapshot's testimony.
type Snapshot = (String, String, String);

/// The decorator attributes a snapshot underlines beneath rustc's
/// `required by a bound in `__nestrs_assert_guard_capability`` note.
///
/// Read from **that note's frame**, not from the whole snapshot. Both narrowings
/// matter and only one is obvious:
///
/// - the frame, because a snapshot renders the offending `#[use_guards]` and the
///   guard's own `struct` in other frames, and reading the file whole credited
///   `use_guards` — inert today only because it is not a binding site;
/// - column 1 inside it, because rustc elides intervening source lines with
///   `...` and prints both an item-level decorator (column 1) and the
///   method-level attribute under it (indented). Two pair halves have never
///   *both* survived the elision — but that is rustc's rendering heuristic and
///   not a contract, and one change to it would let a single snapshot fill both
///   of a crate's cells again, which is the collapse this join was rekeyed to
///   remove.
fn underlined_decorators(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_frame = false;
    for line in text.lines() {
        if line.starts_with("note: required by a bound in `__nestrs_assert_guard_capability`") {
            in_frame = true;
            continue;
        }
        // The frame ends at the next diagnostic header. Matched on a lowercase
        // keyword at column 0 (`note:`, `help:`, `error`, `warning`) rather than
        // on indentation, because two things *inside* a frame also start at
        // column 0: a rendered source line (`23 | #[routes]`) and rustc's `...`
        // elision marker.
        if in_frame && line.starts_with(|c: char| c.is_ascii_lowercase()) {
            in_frame = false;
        }
        if !in_frame {
            continue;
        }
        // A rendered source line is `NN | <source>`; take what follows the bar.
        let Some((_, source)) = line.split_once('|') else {
            continue;
        };
        let Some(rest) = source.strip_prefix(" #[") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
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
    let snapshots = snapshotted_sites(&root, &markers);

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
                snapshots.contains(&(
                    site.krate.clone(),
                    site.decorator.clone(),
                    site.marker.clone(),
                )),
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
