//! The events join: every `warn`+ event the framework emits, against every
//! string a suite asserts.
//!
//! `CLAUDE.md` calls these "the events queried under incident" and requires each
//! to carry structured metadata. Nothing asked whether any test ever *reads*
//! one. The answer was 44 of 149, and the gap is not theoretical: a scheduler
//! event carried `<non-string panic payload>` in place of every job's panic
//! message, through a green suite, because the test that ran that line asserted
//! the job kept firing and never looked at what was logged.
//!
//! Members are derived, never listed. Coverage is checked **workspace-wide** —
//! `demo/` included — because a member asserted from another crate is asserted,
//! and a per-crate view manufactures holes that get closed with duplicate tests.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    is_cfg_test, normalize, parsed, relative, repo_root, rust_files,
};
use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;
use syn::{Attribute, ItemFn, ItemMod, LitStr, Macro};

const BASELINE: &str = "events-baseline.txt";

/// Below this the scan is reading the wrong tree and every "hole" it reports is
/// an artefact. A join that can silently find nothing is worse than no join.
const FLOOR: usize = 100;

/// The literals a macro call spells at its top level, in order. Nested groups
/// are sigils and expressions (`?err`, `%path`), never the message.
fn top_level_literals(tokens: &TokenStream) -> Vec<String> {
    tokens
        .clone()
        .into_iter()
        .filter_map(|t| match t {
            TokenTree::Literal(lit) => syn::parse_str::<LitStr>(&lit.to_string())
                .ok()
                .map(|s| normalize(&s.value())),
            _ => None,
        })
        .collect()
}

fn declared_target(tokens: &TokenStream) -> Option<String> {
    let flat: Vec<TokenTree> = tokens.clone().into_iter().collect();
    flat.windows(3).find_map(|w| match (&w[0], &w[1], &w[2]) {
        (TokenTree::Ident(i), TokenTree::Punct(p), TokenTree::Literal(lit))
            if i == "target" && p.as_char() == ':' =>
        {
            syn::parse_str::<LitStr>(&lit.to_string())
                .ok()
                .map(|s| s.value())
        }
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    target: String,
    message: String,
}

impl Event {
    fn key(&self) -> String {
        format!("{} :: {}", self.target, self.message)
    }
}

#[derive(Default)]
struct Emissions {
    events: Vec<Event>,
}

impl<'ast> Visit<'ast> for Emissions {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if is_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
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
        if name == "warn" || name == "error" {
            self.take(&node.tokens);
        }
        // A decorator emits into the *developer's* code, so its events are the
        // framework's all the same — and `syn::visit` stops at a macro's tokens,
        // which is where every one of them lives. Without this the whole
        // emission site is outside the family: today that is one `warn` in
        // `nest-rs-queue-macros`, which is asserted but was never *required* to
        // be, and tomorrow it is whatever the next decorator writes.
        if name == "quote" {
            self.take_nested(node.tokens.clone());
        }
        syn::visit::visit_macro(self, node);
    }
}

impl Emissions {
    /// Record the event a `warn!`/`error!` argument list spells, if it spells
    /// one at the call site.
    fn take(&mut self, tokens: &TokenStream) {
        let literals = top_level_literals(tokens);
        let target = declared_target(tokens);
        // With a `target:`, the first literal is that target; a call carrying
        // nothing else spells its message as a shared constant, and the call
        // site has no string to compare.
        let needed = if target.is_some() { 2 } else { 1 };
        if literals.len() >= needed
            && let Some(message) = literals.last()
            && !message.is_empty()
        {
            self.events.push(Event {
                target: target.unwrap_or_else(|| "(inherited)".to_owned()),
                message: message.clone(),
            });
        }
    }

    /// Walk a `quote!` body as tokens, taking every `warn!`/`error!` it writes.
    ///
    /// Tokens rather than `syn`, because a `quote!` body is not required to
    /// parse — it is a template, and interpolations (`#ident`) are not Rust.
    fn take_nested(&mut self, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for window in trees.windows(3) {
            let [
                TokenTree::Ident(name),
                TokenTree::Punct(bang),
                TokenTree::Group(args),
            ] = window
            else {
                continue;
            };
            if bang.as_char() == '!' && matches!(name.to_string().as_str(), "warn" | "error") {
                self.take(&args.stream());
            }
        }
        for tree in trees {
            if let TokenTree::Group(group) = tree {
                self.take_nested(group.stream());
            }
        }
    }
}

/// Every string a suite spells. `require_test` is what separates the two uses of
/// one visitor: a file under `tests/` is a suite whole, while a file under
/// `src/` is a suite only inside its `#[cfg(test)]` items.
struct Asserted {
    out: BTreeSet<String>,
    require_test: bool,
    in_test: bool,
}

impl<'ast> Visit<'ast> for Asserted {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        let outer = self.in_test;
        self.in_test |= is_cfg_test(&node.attrs);
        syn::visit::visit_item_mod(self, node);
        self.in_test = outer;
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let outer = self.in_test;
        self.in_test |= is_cfg_test(&node.attrs);
        syn::visit::visit_item_fn(self, node);
        self.in_test = outer;
    }

    /// Attributes are skipped whole, and that is the point: `syn` lowers `///`
    /// and `//!` to `#[doc = "…"]`, whose payload is a `LitStr` like any other.
    /// A module doc quoting the message a suite *used to* assert would then keep
    /// the cell green after the test was deleted — the exact silence this join
    /// exists to break, and reachable with no `mod` declaration at all, since
    /// the corpus walks the directory rather than the module tree.
    ///
    /// Nothing else is lost by it: a `#[should_panic(expected = "…")]` reads a
    /// panic message, never a `tracing` event.
    fn visit_attribute(&mut self, _node: &'ast Attribute) {}

    fn visit_lit_str(&mut self, node: &'ast LitStr) {
        if !self.require_test || self.in_test {
            self.out.insert(normalize(&node.value()));
        }
    }
}

fn emitted_events(root: &Path) -> Vec<Event> {
    let mut scan = Emissions::default();
    for path in rust_files(&root.join("crates")) {
        let rel = relative(&path, root);
        // `cli/src/templates/` is the *generated project's* source. Its events
        // belong to the scaffolded app's family and are covered by the CLI's
        // own suites, not by this one.
        if rel.contains("/tests/") || rel.contains("cli/src/templates/") {
            continue;
        }
        if let Some(file) = parsed(&path) {
            scan.visit_file(&file);
        }
    }
    scan.events
}

fn asserted_strings(root: &Path) -> BTreeSet<String> {
    let mut sources = rust_files(&root.join("crates"));
    sources.extend(rust_files(&root.join("demo")));

    let mut out = BTreeSet::new();
    for path in sources {
        let Some(file) = parsed(&path) else {
            continue;
        };
        let mut scan = Asserted {
            out: BTreeSet::new(),
            require_test: !relative(&path, root).contains("/tests/"),
            in_test: false,
        };
        scan.visit_file(&file);
        out.extend(scan.out);
    }
    out
}

/// A suite may assert a distinctive fragment rather than the whole sentence, and
/// that reads the event just as well. Short fragments are not accepted: a common
/// word would mark every event covered and the join would go quiet.
fn is_asserted(message: &str, corpus: &BTreeSet<String>) -> bool {
    corpus.contains(message)
        || corpus
            .iter()
            .any(|lit| lit.len() >= 24 && message.contains(lit.as_str()))
}

#[test]
fn every_warn_plus_event_is_read_by_a_test() {
    let root = repo_root();
    let events = emitted_events(&root);
    baseline::floor(events.len(), FLOOR, "warn+ events");

    let corpus = asserted_strings(&root);
    let holes: BTreeSet<String> = events
        .iter()
        .filter(|e| !is_asserted(&e.message, &corpus))
        .map(Event::key)
        .collect();

    baseline::gate(
        BASELINE,
        &holes,
        events.len(),
        "events",
        "warn+ event(s)",
        "an event no test names",
    );
}
