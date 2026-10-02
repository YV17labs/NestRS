//! The filters join: every `tracing` target both workspaces emit, against
//! `EnvFilter`'s idea of what a directive naming one of them selects.
//!
//! An operator's only handle on a target is a filter directive, and
//! `EnvFilter` matches one with `starts_with` on the **raw string** rather than
//! on `::` segments (`tracing-subscriber`'s `Directive::cares_about`). So a
//! target that is a prefix of another is not a naming preference — it is a
//! directive that cannot address the first without also addressing the second,
//! and the operator gets no sign that it did.
//!
//! That shipped. `nest_rs::access` carried the one line every edge files per
//! unit of work, and `nest_rs::access_graph` carried the boot `warn` naming
//! resolvers unreachable from the GraphQL schema. The toggle the docs handed
//! out — `<PREFIX>_LOG=info,nest_rs::access=off` — silenced both, so quieting
//! the access log cost a startup diagnostic with nothing on the console to say
//! so. Renaming the family's target to `nest_rs::operation` closed that pair;
//! neither old name survives (that warn is `nest_rs::graphql`'s, filed by the
//! crate that owns the resolver registry). This join is what stops the next
//! one.
//!
//! Members are derived, never listed: every target spelled at a `tracing`
//! emission site under `crates/` and `demo/`. The pair, not the target, is the
//! hole — a prefix relation belongs to two names and neither is wrong on its
//! own.
//!
//! **What it reads by its spelling**: the `target:` of every macro call. It
//! reads calls, not `macro_rules!` transcribers, so a literal target a
//! transcriber writes is refused by the `blinds` join — a target handed *to* a
//! macro is read where the macro is called.
//!
//! **And how each one is spelled.** `CLAUDE.md` makes a target a constant its
//! owner declares, never a literal at the call site, because a typo in a literal
//! is a target carrying one event no filter selects, and nothing says so. A
//! framework crate declares `TARGET` at its root; a product feature at its
//! module's, `features::posts::TARGET`. The second test holds every call site of
//! both workspaces to it — shipped code and the doc examples it carries, since an
//! example is the line a reader copies — and leaves out test code, whose targets
//! are fixtures. The demo logged on eighteen literals until 7.0, every one a
//! `features::…` string retyped per file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    Named, declared_target, declared_targets, doctests, each_source, is_cfg_test, item_attrs,
    named_at, repo_root, top_level, value_after,
};
use syn::Macro;
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![Followed::key("target")]
}

/// Below this the scan is reading the wrong tree and every pair it fails to
/// report is a pair nobody will look for again.
const FLOOR: usize = 20;

/// Every target both workspaces emit, each with the files that spell it, plus
/// the paths the resolver could not place.
#[derive(Default)]
struct Scan {
    file: String,
    found: BTreeMap<String, BTreeSet<String>>,
    unresolved: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_macro(&mut self, node: &'ast Macro) {
        match declared_target(&node.tokens, &self.file) {
            // A literal here is either a product target (`features::users`) or
            // a test fixture; a framework one resolved to its value.
            Some(Named::Literal(target)) => {
                self.found
                    .entry(target)
                    .or_default()
                    .insert(self.file.clone());
            }
            // A constant the table could not place: reported, never skipped.
            Some(Named::Path(path)) => {
                self.unresolved.insert(format!("{path} ({})", self.file));
            }
            None => {}
        }
        syn::visit::visit_macro(self, node);
    }
}

fn emitted_targets(root: &Path) -> (BTreeMap<String, BTreeSet<String>>, BTreeSet<String>) {
    let mut scan = Scan::default();
    each_source(root, |rel, ast| {
        scan.file = rel.to_owned();
        scan.visit_file(ast);
    });
    // Floored on what the **walk** found, before the declarations are merged.
    // Merging first made this unfireable — 27 constants clear any floor on their
    // own, so a source walk that broke would have left the join green, which is
    // the one failure `baseline::floor` exists to prevent.
    baseline::floor(scan.found.len(), FLOOR, "tracing target(s) at a call site");

    // The population is the declarations as well as the call sites: an unused
    // constant is still a name a directive selects, and it is exactly where the
    // next prefix pair gets introduced without anyone seeing it.
    for (target, krate, _) in declared_targets() {
        scan.found
            .entry((*target).to_owned())
            .or_default()
            .insert(format!("crates/{krate}"));
    }
    (scan.found, scan.unresolved)
}

/// Where a filter directive naming `outer` also selects `inner`.
///
/// Equal strings are the same target, not a pair; `starts_with` is what
/// `EnvFilter` does, so this is that predicate and not a segment-aware
/// approximation of it.
fn swallows(outer: &str, inner: &str) -> bool {
    outer != inner && inner.starts_with(outer)
}

#[test]
fn no_target_is_a_prefix_of_another() {
    let root = repo_root();
    let (targets, unresolved) = emitted_targets(&root);

    assert!(
        unresolved.is_empty(),
        "{} emission site(s) name a target through a path this join cannot \
         resolve, so those targets are outside the set it checks. Either spell \
         the target as a literal, or teach `resolve_target` about the \
         constant:\n  {}",
        unresolved.len(),
        unresolved.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );

    let names: Vec<&String> = targets.keys().collect();
    let spelled_at = |target: &String| {
        targets[target]
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut pairs = BTreeSet::new();
    for outer in &names {
        for inner in &names {
            if swallows(outer, inner) {
                pairs.insert(format!(
                    "`{outer}` swallows `{inner}`\n      {outer} at {}\n      {inner} at {}",
                    spelled_at(outer),
                    spelled_at(inner),
                ));
            }
        }
    }

    // No baseline: the family had exactly one such pair, it was the reason this
    // join was written, and it was closed in the same change. A recorded hole
    // here would be a directive documented to select one target while selecting
    // two, which is not a state to grandfather.
    assert!(
        pairs.is_empty(),
        "{} of the {} target(s) emitted across the two workspaces cannot be \
         named by a filter directive without naming another, because \
         `EnvFilter` matches a directive by `starts_with` on the raw string \
         rather than by `::` segment. Rename one side of each pair:\n  {}",
        pairs.len(),
        targets.len(),
        pairs.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );
}

/// The call sites that spell their target as a literal, and how many sites were
/// read at all.
#[derive(Default)]
struct LiteralSites {
    file: String,
    read: usize,
    found: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for LiteralSites {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if is_cfg_test(item_attrs(node)) {
            return;
        }
        syn::visit::visit_item(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        let flat = top_level(&node.tokens);
        if let Some(at) = value_after(&flat, "target", ':') {
            self.read += 1;
            if let Some((Named::Literal(text), _)) = named_at(&flat, at) {
                self.found.insert(format!("{} spells `{text}`", self.file));
            }
        }
        syn::visit::visit_macro(self, node);
    }
}

#[test]
fn no_target_is_spelled_as_a_literal_where_it_is_emitted() {
    let root = repo_root();
    let mut scan = LiteralSites::default();
    each_source(&root, |rel, ast| {
        if rel.contains("/tests/") || !rel.contains("/src/") {
            return;
        }
        scan.file = rel.to_owned();
        scan.visit_file(ast);
        for example in doctests(ast) {
            scan.visit_file(&example);
        }
    });
    baseline::floor(scan.read, FLOOR, "tracing target(s) at a call site");
    assert!(
        scan.found.is_empty(),
        "{} call site(s) spell their target as a literal. A target is a constant \
         its owner declares — `TARGET` at a framework crate's root, at a product \
         feature's module root — and the call site names it:\n  {}",
        scan.found.len(),
        scan.found.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );
}
