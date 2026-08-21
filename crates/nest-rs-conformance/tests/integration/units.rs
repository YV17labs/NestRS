//! The units join: every unit of work the framework opens, against the one
//! place its canonical name is allowed to come from.
//!
//! A unit of work has **one** name — `<edge>.<unit>`, declared as
//! `<owning crate>::unit::<UNIT>` — and that name is read three times: by the
//! `operation_span!` that opens the unit, by its operation line's `name:`, and
//! by that line's `message`. Sharing a constant is what makes the three agree;
//! this join is what stops a fourth site from spelling a fourth string.
//!
//! The names live **with their edges**, not in the kernel: a unit name is
//! per-edge vocabulary exactly as a span target is, so `nest_rs_core` holds the
//! grammar (`operation_log`) and each edge holds its own name. That is what
//! moves the shape and namespace checks here — a hand-written member list in
//! the kernel could only ever police the names the kernel could see.
//!
//! It had to: the spans said `http.request` and `mcp.operation` while the lines
//! said `request served` and `operation served`, two vocabularies for six
//! things, and `nest-rs-ws` carried the workaround in a comment — "`lifecycle`
//! rather than the span's name because `tracing` offers no way to read one
//! back". Nothing failed while that was true, which is the whole argument for
//! checking it here rather than trusting the next author to notice.
//!
//! Members are derived, never listed: every `operation_span!` call site and
//! every `tracing::info!` whose target is `operation_log::TARGET`, under
//! `crates/` and `demo/` — **inside a doctest as well as in an item**, since the
//! example is what a developer copies and it is the site this join reached
//! last.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    Named, declared_units, doctests, each_source, is_cfg_test, named_at, operation_log_target,
    past_value, repo_root, resolve_target, top_level, value_after,
};
use proc_macro2::TokenTree;
use syn::Macro;
use syn::visit::Visit;

/// Below this the scan is reading the wrong tree, and a join that finds nothing
/// reads exactly like a join that found nothing wrong.
const FLOOR: usize = 8;

/// The two vocabularies a site may name, and the module each lives in.
///
/// Checking the **module** rather than merely "is it a path" is what makes the
/// failure sentences true: `crate::whatever::REQUEST` used to satisfy a message
/// saying the name must come from a `unit` module. The module is checked and
/// the *crate* is not, deliberately: which crate owns an edge is the edge's
/// question, and the shape check below is what holds the value to the closed
/// namespace whoever declared it.
const UNIT_MODULE: &str = "unit";
const KIND_MODULE: &str = "kind";
/// Not a vocabulary this join classifies — the operation line's `target:` is
/// resolved rather than classified. It labels the one refusal this join owes
/// about a target, so the sentence names the module a target comes from rather
/// than the one a unit name does.
const TARGET_MODULE: &str = "target";

/// What a slot that named nothing at all is recorded as.
///
/// A [`Spelled::Binding`] like any other, so the refusal it lands in is the one
/// worded for a name the join cannot follow — the case a shared emitter is
/// waived for, and the case an edge that forgot the slot fails on.
const ABSENT: &str = "<absent>";

/// How a site named its unit, once the shared reader has classified it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Spelled {
    /// A path into a `unit` module — the only conformant form. The payload is
    /// the constant's name.
    Unit(String),
    /// A path that is not one, named as written.
    Elsewhere(String),
    /// A string literal: a second spelling of a name that already exists.
    Literal(String),
    /// A local binding, or nothing at all. Legal only at the shared emitter.
    Binding(String),
}

/// Whether this `tracing::info!` files on `operation_log::TARGET`.
///
/// Resolved to the **value**, never matched on the constant's name: every crate
/// spells its own concern `TARGET` too, so `target: crate::TARGET` in
/// `nest-rs-social` is an ordinary event, and reading the name alone claimed
/// eight of them as operation lines.
fn is_operation_line(tokens: &[TokenTree], file: &str) -> Membership {
    let Some(at) = value_after(tokens, "target", ':') else {
        // No `target:` at all is an ordinary event — the operation line always
        // names one, so nothing is dropped here.
        return Membership::Outside;
    };
    match named_at(tokens, at) {
        Some((Named::Path(_), segments)) => {
            if resolve_target(&segments, file) == operation_log_target() {
                Membership::Family
            } else {
                Membership::Outside
            }
        }
        // **A literal target used to leave the population instead of failing
        // it**, which is the one direction a conformance join may never go: an
        // `info!(target: "nest_rs::operation", …)` was never classified, so its
        // `name:` and `message` were free to spell a fourth string for a unit of
        // work and this join stayed green. A literal *equal to* the family's
        // target is a member — and is itself reported, since a target the
        // framework interprets is a constant declared by its owner. A literal
        // naming anything else is an ordinary event, and whether it should have
        // been a constant is the `filters` join's subject rather than this
        // one's.
        Some((Named::Literal(text), _)) if Some(text.as_str()) == operation_log_target() => {
            Membership::FamilyByLiteral
        }
        _ => Membership::Outside,
    }
}

/// Whether an `info!` belongs to the operation-line family, and how it said so.
enum Membership {
    /// Not an operation line.
    Outside,
    /// An operation line, naming the family's target through its constant.
    Family,
    /// An operation line whose target is spelled as a literal.
    FamilyByLiteral,
}

/// Classify the value at `at` against the module it is required to come from.
fn spelled_at(tokens: &[TokenTree], at: usize, module: &str) -> Option<Spelled> {
    let (named, segments) = named_at(tokens, at)?;
    Some(match named {
        Named::Literal(text) => Spelled::Literal(text),
        Named::Path(last) if segments.len() == 1 => Spelled::Binding(last),
        Named::Path(last) if segments.iter().any(|s| s == module) => Spelled::Unit(last),
        Named::Path(_) => Spelled::Elsewhere(segments.join("::")),
    })
}

#[derive(Default)]
struct Scan {
    file: String,
    /// `(site, the module it must read, how it named its value)` → the files.
    ///
    /// The module travels in the key rather than being recovered from the site
    /// when the failure is worded: a site's vocabulary is decided once, where
    /// [`spelled_at`] is called, and a second mapping beside the sentence is
    /// what let every refusal say `unit` — including the ones about a span
    /// kind.
    found: BTreeMap<(&'static str, &'static str, Spelled), BTreeSet<String>>,
}

impl Scan {
    fn record(&mut self, site: &'static str, module: &'static str, named: Spelled) {
        self.found
            .entry((site, module, named))
            .or_default()
            .insert(self.file.clone());
    }
}

impl<'ast> Visit<'ast> for Scan {
    // A `#[cfg(test)]` emission opens no unit of work — it renders a line so a
    // formatter can be asserted against it, which is what
    // `nest_rs_core::logging`'s tests do and why the kernel spells a fixture
    // name there. Correcting the population, not waiving a member: every real
    // site is compiled into the shipped crate, and a transport that hid one
    // behind `cfg(test)` would be shipping nothing.
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if is_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if is_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        let name = node
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        // Named before flattened: `top_level` clones the stream, and every
        // `assert!`, `format!` and `quote!` body in both workspaces reaches
        // this visitor. Only these two macro names are ever read.
        if name != "operation_span" && name != "info" {
            syn::visit::visit_macro(self, node);
            return;
        }
        let tokens = top_level(&node.tokens);

        if name == "operation_span" {
            // `operation_span!(target: …, kind: …, <name>, &correlation, …)`.
            // The unit's name is the positional argument after the kind, and the
            // kind is a path now, so the value has to be *walked* rather than
            // stepped over — a fixed offset read the middle of it.
            if let Some(kind_at) = value_after(&tokens, "kind", ':') {
                if let Some(named) = spelled_at(&tokens, kind_at, KIND_MODULE) {
                    self.record("kind:", KIND_MODULE, named);
                }
                if let Some(named) = spelled_at(&tokens, past_value(&tokens, kind_at), UNIT_MODULE)
                {
                    self.record("operation_span!", UNIT_MODULE, named);
                }
            }
        } else {
            // Membership first, then the refusal it owes, then the slots — three
            // steps, read as three. Deciding and recording inside one `&&`
            // operand hid a mutation in a condition.
            let membership = is_operation_line(&tokens, &self.file);
            if matches!(membership, Membership::FamilyByLiteral) {
                let target = operation_log_target().unwrap_or_default().to_owned();
                self.record("target:", TARGET_MODULE, Spelled::Literal(target));
            }
            if matches!(membership, Membership::Outside) {
                syn::visit::visit_macro(self, node);
                return;
            }
            // Both slots carry the same value under the same rule, so they are
            // read by one loop: a third one is a row, not a third spelling.
            // Absent is legal only at a shared emitter — see below.
            for (site, key, punct) in [("message", "message", '='), ("name:", "name", ':')] {
                let named = value_after(&tokens, key, punct)
                    .and_then(|at| spelled_at(&tokens, at, UNIT_MODULE))
                    .unwrap_or_else(|| Spelled::Binding(ABSENT.to_owned()));
                self.record(site, UNIT_MODULE, named);
            }
        }
        syn::visit::visit_macro(self, node);
    }
}

fn scan(root: &Path) -> BTreeMap<(&'static str, &'static str, Spelled), BTreeSet<String>> {
    let mut scan = Scan::default();
    each_source(root, |rel, ast| {
        scan.file = rel.to_owned();
        scan.visit_file(ast);
        // The example a developer copies is a member of this family too. It was
        // not, and the canonical `operation_span!` doctest spelled all three
        // slots as literals for exactly as long — the one site teaching the
        // grammar was the one site never read.
        scan.file = format!("{rel} (doctest)");
        for example in doctests(ast) {
            scan.visit_file(&example);
        }
    });
    scan.found
}

/// The one emitter that may name its unit from a binding, and the reason.
///
/// `under_connection` serves `ws.connect` and `ws.disconnect` from one body, and
/// a line's `name:` is baked into the callsite's `static` metadata — it cannot
/// read a parameter. So that site takes the canonical name as an argument and
/// spends it on `message` alone. Stated here rather than waived silently,
/// because "a site that could not follow" is exactly the shape a real hole
/// hides in.
fn is_shared_emitter(files: &BTreeSet<String>) -> bool {
    files
        .iter()
        .all(|f| f.ends_with("nest-rs-ws/src/gateway.rs"))
}

#[test]
fn every_unit_of_work_is_named_by_the_shared_constant() {
    let root = repo_root();
    let found = scan(&root);
    baseline::floor(found.len(), FLOOR, "unit-naming site(s)");

    let mut wrong: Vec<String> = Vec::new();
    for ((site, module, named), files) in &found {
        let where_ = files.iter().cloned().collect::<Vec<_>>().join(", ");
        match named {
            Spelled::Unit(_) => {}
            Spelled::Literal(text) => wrong.push(format!(
                "{site} spells `{text}` as a literal ({where_}) — it must read a \
                 constant from its owning crate's `{module}` module",
            )),
            Spelled::Elsewhere(path) => wrong.push(format!(
                "{site} names `{path}` ({where_}), which is not a constant in a \
                 `{module}` module",
            )),
            Spelled::Binding(_) if is_shared_emitter(files) => {}
            Spelled::Binding(text) => wrong.push(format!(
                "{site} reads `{text}` ({where_}), which this join cannot trace \
                 back to a `{module}` module",
            )),
        }
    }
    wrong.sort();
    assert!(
        wrong.is_empty(),
        "{} unit-naming site(s) do not read the shared constant, so a unit of \
         work can be called two things:\n  {}",
        wrong.len(),
        wrong.join("\n  "),
    );

    // Every name **used at either site** is opened by a span and filed by a
    // line — which is not quite "every declared name", and the difference is
    // stated because it is a gap: a constant declared in a `unit` module and
    // never used at either site is in neither set and passes here. The sibling
    // test below is the one that reads `declared_units()`, so a name that
    // exists is at least held to the grammar; that it is *emitted* is what this
    // half cannot see. Today the two populations coincide (six declarations,
    // six used), which is exactly why it is invisible.
    let by_site = |site: &str| -> BTreeSet<String> {
        found
            .keys()
            .filter(|(s, _, _)| *s == site)
            .filter_map(|(_, _, n)| match n {
                Spelled::Unit(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    };
    let spans = by_site("operation_span!");
    let messages = by_site("message");

    let unopened: Vec<String> = messages.difference(&spans).cloned().collect();
    let unfiled: Vec<String> = spans.difference(&messages).cloned().collect();
    assert!(
        unopened.is_empty(),
        "unit(s) filed on a line with no `operation_span!` opening them: {}",
        unopened.join(", "),
    );
    // Keyed on the constant's **name**, which is what a site spells, and that is
    // a limit worth stating: two crates may each declare a `MESSAGE`, and the
    // namespace check in the sibling test forces their *values* apart
    // (`ws.message` vs `events.message`) while placing no constraint on the
    // identifier. A future `nest_rs_events::unit::MESSAGE` opened as a span with
    // no line would be cross-satisfied by `nest_rs_ws::unit::MESSAGE`'s line —
    // verbatim the scenario this half exists to catch. The disambiguating pair
    // is `declared_units()`'s `(crate, constant)`; using it needs the *site* to
    // carry the declaring crate too, which `spelled_at` does not resolve, so it
    // is stated here rather than half-built.
    //
    // A unit whose span is opened at the shared emitter files its line through
    // that emitter's binding, so it never reaches `message` under its own name.
    // Derived from the same predicate as the waiver above rather than re-typed
    // as a list of constant names: two encodings of one fact drift apart, and
    // the name list would have blessed any future unit that happened to be
    // called `WS_CONNECT` — in the join that forbids literals.
    let waived = |unit: &String| {
        found
            .get(&("operation_span!", UNIT_MODULE, Spelled::Unit(unit.clone())))
            .is_some_and(is_shared_emitter)
    };
    let anonymous: Vec<String> = unfiled.into_iter().filter(|u| !waived(u)).collect();
    assert!(
        anonymous.is_empty(),
        "unit(s) opened as a span that file no operation line, so their work is \
         anonymous on the console: {}",
        anonymous.join(", "),
    );
}

/// The closed edge vocabulary (`architecture.md`), the only set a canonical
/// name may take its namespace from.
///
/// The one list here that is **stated rather than derived**, and it has to be:
/// the vocabulary is closed by an owner decision recorded in prose, so there is
/// nothing in the tree to read it off — a crate named `nest-rs-grpc` would prove
/// only that someone wrote one. Opening an edge therefore touches
/// `architecture.md` and this line, which is the deliberate, reviewed act the
/// closure exists to require. Shared with the `naming` join, which reads the
/// same vocabulary to tell an edge adapter from a module-root role file — a
/// second copy there would have made the reviewed act two lines. What *is* derived is everything the list is
/// checked against: the members below come from the declarations themselves.
pub(crate) const EDGES: [&str; 7] = [
    "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
];

/// Below this the scan is reading the wrong tree — six edges declare a unit
/// today and one of them declares three.
const DECLARED_FLOOR: usize = 6;

/// Every declared name is `<edge>.<unit>`, and it says who owns it.
///
/// This lived in `nest-rs-core` as a hand-written array for as long as the
/// kernel held the names — the shape `testing.md` names as the defect, and one
/// that could only ever police the members whoever typed it remembered. The
/// names moved to their edges; the rule moved here, over a population read out
/// of the source.
#[test]
fn every_declared_unit_name_is_an_edge_namespace_that_names_its_owner() {
    let declared = declared_units();
    baseline::floor(declared.len(), DECLARED_FLOOR, "declared unit name(s)");

    let mut wrong: Vec<String> = Vec::new();
    for (name, krate, konst) in &declared {
        let at = format!("`{konst}` in {krate}");
        let Some((namespace, tail)) = name.split_once('.') else {
            wrong.push(format!(
                "{at} declares `{name}`, which is not `<edge>.<unit>`"
            ));
            continue;
        };
        if !EDGES.contains(&namespace) {
            wrong.push(format!(
                "{at} declares `{name}`, whose namespace is outside the closed edge \
                 vocabulary; open `{namespace}` as an edge in `architecture.md` first",
            ));
        }
        // The namespace names the edge, and the crate that owns the edge is the
        // crate that declares the name — so the two cannot disagree without one
        // of them lying about who a unit of work belongs to. Derived, unlike the
        // vocabulary above: it is read off the path the declaration sits at.
        else if krate.strip_prefix("nest-rs-") != Some(namespace) {
            wrong.push(format!(
                "{at} declares `{name}`, but `{namespace}` is another crate's edge — a unit \
                 name is declared by the crate that owns it",
            ));
        }
        if tail.is_empty() || tail.contains('.') {
            wrong.push(format!(
                "{at} declares `{name}`, which must carry exactly one dot"
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
        {
            wrong.push(format!("{at} declares `{name}`, which must be lowercase"));
        }
    }
    let distinct: BTreeSet<&String> = declared.iter().map(|(name, _, _)| name).collect();
    if distinct.len() != declared.len() {
        wrong.push(format!(
            "two units of work share one canonical name, so nothing can tell them apart: {} \
             declarations for {} names",
            declared.len(),
            distinct.len(),
        ));
    }

    wrong.sort();
    assert!(
        wrong.is_empty(),
        "{} declared unit name(s) are off the grammar `nest_rs_core::operation_log` \
         states:\n  {}",
        wrong.len(),
        wrong.join("\n  "),
    );
}
