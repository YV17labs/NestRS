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
    Named, declared_target, is_cfg_test, normalize, parsed, relative, repo_root, rust_files,
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
///
/// The `bool` is `true` when the literal is a **fragment** — one piece of a
/// `concat!` whose other pieces the call site supplies — rather than the whole
/// message. It decides which matching rule the event may take: a whole message
/// must be asserted whole, a fragment can only ever be *inside* what a console
/// shows. See [`is_asserted`].
fn top_level_literals(tokens: &TokenStream) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for (i, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Literal(lit) => {
                if let Ok(s) = syn::parse_str::<LitStr>(&lit.to_string()) {
                    out.push((normalize(&s.value()), false));
                }
            }
            // `::core::concat!("skipped ", $what, ": …")` in the message
            // position. A macro that serves several edges from one body writes
            // its sentence this way — `report_inert_host!` does, for five of
            // them — and the assembled message is not a literal at any site, so
            // a top-level-only scan saw an event with no message and dropped it.
            // The fragments are what a suite can assert; the longest is what
            // `is_asserted`'s length guard is about.
            TokenTree::Group(group)
                if i > 0
                    && matches!(&trees[i - 1], TokenTree::Punct(p) if p.as_char() == '!')
                    && trees[..i]
                        .iter()
                        .rev()
                        .nth(1)
                        .is_some_and(|t| matches!(t, TokenTree::Ident(id) if id == "concat")) =>
            {
                let mut fragments = top_level_literals(&group.stream());
                fragments.sort_by_key(|(f, _)| std::cmp::Reverse(f.len()));
                out.extend(fragments.into_iter().take(1).map(|(f, _)| (f, true)));
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    target: String,
    message: String,
    /// The message is one piece of a `concat!` the call site completes, so what
    /// a console shows is longer than this. See [`is_asserted`].
    fragment: bool,
}

impl Event {
    fn key(&self) -> String {
        format!("{} :: {}", self.target, self.message)
    }
}

#[derive(Default)]
struct Emissions {
    file: String,
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
        // `warn!` / `error!` by name. `tracing::event!(Level::WARN, …)` is the
        // documented alternative spelling and is **not** in this population —
        // nothing writes one today (checked), so it is a channel rather than a
        // hole, and it is named here because a population that loses a member
        // silently is the one direction this join may not fail in.
        if name == "warn" || name == "error" {
            self.take(&node.tokens);
        }
        // A decorator emits into the *developer's* code, so its events are the
        // framework's all the same — and `syn::visit` stops at a macro's tokens,
        // which is where every one of them lives. Without this the whole
        // emission site is outside the family: today that is one `warn` in
        // `nest-rs-queue-macros`, which is asserted but was never *required* to
        // be, and tomorrow it is whatever the next decorator writes.
        // A `macro_rules!` body is the same case as a `quote!` one: the events
        // are the framework's, and `syn::visit` stops at the tokens.
        // `report_inert_host!` is the live instance — one definition, two arms,
        // five call sites across five crates, and the whole inert-discovery
        // report `framework.md` makes mandatory. None of it was in the
        // population: the call sites carry no literal at all, and the definition
        // was never descended into.
        if name == "quote" || name == "macro_rules" {
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
        let named = declared_target(tokens, &self.file);
        // A target spelled as a **literal** occupies the first slot, so the
        // message is the second; a constant occupies none, and the message is
        // the only literal there is. Keying this off "did it resolve" instead of
        // "was it written as a string" dropped two thirds of this family the day
        // the targets became constants, and the floor is what caught it.
        let written_as_string = matches!(named, Some(Named::Literal(_)))
            && literals.first().is_some_and(
                |(first, _)| matches!(&named, Some(Named::Literal(t)) if normalize(t) == *first),
            );
        let needed = usize::from(written_as_string) + 1;
        if literals.len() >= needed
            && let Some((message, fragment)) = literals.last()
            && !message.is_empty()
        {
            self.events.push(Event {
                target: match named {
                    Some(Named::Literal(target)) => target,
                    // A constant the table could not place, or no `target:` at
                    // all: the call inherits its module's, which this join
                    // records as such rather than guessing.
                    _ => "(inherited)".to_owned(),
                },
                message: message.clone(),
                fragment: *fragment,
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
        if let Some(file_ast) = parsed(&path) {
            scan.file = rel;
            scan.visit_file(&file_ast);
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

/// Whether some suite reads this event.
///
/// **A whole message must be asserted whole, or by a long fragment of itself.**
/// A suite writing `logs.find(target, "…")` with a distinctive substring reads
/// the event just as well, and the 24-character floor is what stops a common
/// word from marking every event covered.
///
/// **The reverse — a corpus literal that *contains* the message — is admitted
/// only for a `concat!` fragment**, and the narrowing is measured rather than
/// cautious. Admitted for every event it made **nine** cells unprovable, five of
/// them `warn`+ authorization and authentication denials, each kept green by a
/// *different* event's assertion: `"transaction commit failed"` swallowed by
/// `"dispatch transaction commit failed"`, `"access denied"` by
/// `"access denied — row outside the caller's scope"`, `"authorization denied"`
/// by `"authorization denied: no ambient ability"`. The length guard does not
/// help there — it bounds the *corpus* literal, so it says nothing about a short
/// message sitting inside a longer sibling's assertion.
///
/// What the direction earns is exactly one cell: the assembled
/// `report_inert_host!` sentence, whose framework half is a fragment and whose
/// middle the call site supplies, so there is no whole message to match. That is
/// the case, and now the only case.
fn is_asserted(event: &Event, corpus: &BTreeSet<String>) -> bool {
    let message = event.message.as_str();
    if corpus.contains(message) {
        return true;
    }
    corpus.iter().any(|lit| {
        lit.len() >= 24
            && (message.contains(lit.as_str()) || (event.fragment && lit.contains(message)))
    })
}

#[test]
fn every_warn_plus_event_is_read_by_a_test() {
    let root = repo_root();
    let events = emitted_events(&root);
    baseline::floor(events.len(), FLOOR, "warn+ events");

    let corpus = asserted_strings(&root);
    let holes: BTreeSet<String> = events
        .iter()
        .filter(|e| !is_asserted(e, &corpus))
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
