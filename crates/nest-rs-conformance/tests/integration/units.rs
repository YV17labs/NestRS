//! The units join: every unit of work the framework opens, against the one
//! place its canonical name is allowed to come from.
//!
//! A unit of work has **one** name — `<edge>.<unit>`, declared in
//! `nest_rs_core::operation_log::unit` — and that name is read three times: by
//! the `operation_span!` that opens the unit, by its operation line's `name:`,
//! and by that line's `message`. Sharing a constant is what makes the three
//! agree; this join is what stops a fourth site from spelling a fourth string.
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
    Named, doctests, each_source, named_at, operation_log_target, past_value, repo_root,
    resolve_target, top_level, value_after,
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
/// failure sentences true: `crate::whatever::HTTP_REQUEST` used to satisfy a
/// message saying the name must come from `operation_log::unit`.
const UNIT_MODULE: &str = "unit";
const KIND_MODULE: &str = "kind";

/// How a site named its unit, once the shared reader has classified it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Spelled {
    /// A path into `operation_log::unit` — the only conformant form. The
    /// payload is the constant's name.
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
fn is_operation_line(tokens: &[TokenTree], file: &str) -> bool {
    let Some(at) = value_after(tokens, "target", ':') else {
        return false;
    };
    let Some((Named::Path(_), segments)) = named_at(tokens, at) else {
        return false;
    };
    resolve_target(&segments, file) == operation_log_target()
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
    /// what let every refusal say `operation_log::unit` — including the ones
    /// about a span kind.
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
    fn visit_macro(&mut self, node: &'ast Macro) {
        let name = node
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
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
        } else if name == "info" && is_operation_line(&tokens, &self.file) {
            if let Some(named) = value_after(&tokens, "message", '=')
                .and_then(|at| spelled_at(&tokens, at, UNIT_MODULE))
            {
                self.record("message", UNIT_MODULE, named);
            } else {
                self.record(
                    "message",
                    UNIT_MODULE,
                    Spelled::Binding("<absent>".to_owned()),
                );
            }
            match value_after(&tokens, "name", ':')
                .and_then(|at| spelled_at(&tokens, at, UNIT_MODULE))
            {
                Some(named) => self.record("name:", UNIT_MODULE, named),
                // Absent is legal only at a shared emitter — see below.
                None => self.record(
                    "name:",
                    UNIT_MODULE,
                    Spelled::Binding("<absent>".to_owned()),
                ),
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
                 constant from `operation_log::{module}`",
            )),
            Spelled::Elsewhere(path) => wrong.push(format!(
                "{site} names `{path}` ({where_}), which is not a constant in \
                 `operation_log::{module}`",
            )),
            Spelled::Binding(_) if is_shared_emitter(files) => {}
            Spelled::Binding(text) => wrong.push(format!(
                "{site} reads `{text}` ({where_}), which this join cannot trace \
                 back to `operation_log::{module}`",
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

    // Every declared name is opened by a span *and* filed by a line. This is
    // the half that catches the transport added tomorrow which opens its unit
    // and forgets the operator ever hears about it.
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
