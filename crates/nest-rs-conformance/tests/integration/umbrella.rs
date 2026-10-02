//! The umbrella join: the two obligations of the front door that only the
//! workspace as a whole can see.
//!
//! - **Every decorator is applied in `nest-rs-macro-hygiene`**, the crate whose
//!   one dependency proves an expansion needs no second manifest line. A
//!   decorator with no use site there is a decorator nothing proves.
//! - **Every edge the umbrella arms is armed on every guard crate.** A
//!   `Guard::check_<edge>` arm compiled out authorises everything at that edge.
//!
//! The rest of *Shipping a new capability* is held elsewhere: the re-export by
//! building `nest-rs-macro-hygiene` once per capability feature, and the README
//! and `## Install` lines by the docs linter (`family-mention`,
//! `readme-install`), which already reads both.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::sources::{
    crate_dirs, exported_decorators, parsed, repo_root, rust_files, umbrella_matrix,
};
use syn::Item;

/// Twenty-seven decorators stand today; below this the scan reads the wrong tree.
const DECORATOR_FLOOR: usize = 20;

/// Four edges arm a guard crate today.
const PAIRINGS_FLOOR: usize = 4;

/// Every attribute `nest-rs-macro-hygiene` applies — attributes, not
/// identifiers, so a `mod resolver;` line does not stand in for `#[resolver]`.
fn hygiene_attrs(root: &Path) -> BTreeSet<String> {
    #[derive(Default)]
    struct Applied(BTreeSet<String>);

    impl<'ast> syn::visit::Visit<'ast> for Applied {
        fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
            if let Some(last) = node.path().segments.last() {
                self.0.insert(last.ident.to_string());
            }
            syn::visit::visit_attribute(self, node);
        }
    }

    let mut applied = Applied::default();
    for path in rust_files(&root.join("crates/nest-rs-macro-hygiene/src")) {
        if let Some(ast) = parsed(&path) {
            syn::visit::Visit::visit_file(&mut applied, &ast);
        }
    }
    applied.0
}

/// Every `#[proc_macro_attribute]` a `crates/*-macros` crate exports is applied
/// in `nest-rs-macro-hygiene`.
#[test]
fn every_decorator_is_applied_in_the_hygiene_witness() {
    let root = repo_root();
    let applied = hygiene_attrs(&root);
    let mut exported = BTreeSet::new();
    for dir in crate_dirs() {
        if dir.starts_with(root.join("crates"))
            && dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("-macros"))
        {
            exported.extend(exported_decorators(&dir));
        }
    }
    assert!(
        exported.len() >= DECORATOR_FLOOR,
        "read {} decorator(s) — the walk is reading the wrong tree",
        exported.len(),
    );

    let unapplied: Vec<&String> = exported.iter().filter(|d| !applied.contains(*d)).collect();
    assert!(
        unapplied.is_empty(),
        "no use site in nest-rs-macro-hygiene, so nothing proves the expansion needs no \
         second manifest line: {unapplied:?}",
    );
}

/// **An edge the umbrella turns on for the guard trait, it turns on for every
/// crate that implements that edge's entry.**
///
/// `Guard::check_graphql` / `check_ws_message` / `check_mcp` each have a default
/// `Ok(())` body, so a guard whose arm is compiled out **authorises everything
/// at that edge, silently**. Two optional crates implement those arms behind
/// their own per-edge features, and the umbrella pairs them by hand:
/// `ws = [.., "nest-rs-guards/ws", "nest-rs-authz?/ws", "nest-rs-throttler?/ws"]`.
/// An audit reproduced the gap the day the pairing became hand-written: same
/// source, `authz = ["http"]` with `guards = ["ws"]`, a message with no ambient
/// ability came back **PASSED** where `HEAD` returned **DENIED 401**.
///
/// The population is derived: a crate implementing `fn check_<x>` owes the
/// pairing, so a new guard crate joins the day it is written.
#[test]
fn every_edge_the_umbrella_arms_is_armed_on_every_guard_crate() {
    // `Guard`'s per-edge entries and the umbrella feature each belongs to.
    const ENTRIES: [(&str, &str); 3] = [
        ("check_graphql", "graphql"),
        ("check_ws_message", "ws"),
        ("check_mcp", "mcp"),
    ];
    let root = repo_root();
    let matrix = umbrella_matrix(&root);
    let mut offenders = Vec::new();
    let mut checked = 0usize;

    for (entry, edge) in ENTRIES {
        // `nest-rs-guards` owns the trait rather than implementing it for a
        // bound guard, and `nest-rs-macro-hygiene` is the witness crate —
        // neither is an optional crate the umbrella pairs.
        let implementors: BTreeSet<String> = crate_dirs()
            .into_iter()
            .filter(|k| k.starts_with(root.join("crates")))
            .filter(|k| {
                rust_files(&k.join("src"))
                    .iter()
                    .filter_map(|p| parsed(p))
                    .any(|f| file_declares_impl_fn(&f, entry))
            })
            .filter_map(|k| k.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .filter(|k| k != "nest-rs-guards" && k != "nest-rs-macro-hygiene")
            .collect();

        let enabled = matrix.entries_of(edge);
        if enabled.is_empty() {
            offenders.push(format!("the umbrella declares no `{edge}` feature"));
            continue;
        }
        for krate in &implementors {
            checked += 1;
            let pairing = format!("{krate}?/{edge}");
            if !enabled.iter().any(|e| e == &pairing) {
                offenders.push(format!(
                    "`{edge}` arms `{entry}` but does not enable `{pairing}` — \
                     that crate's guard would authorise every {edge} unit in silence",
                ));
            }
        }
    }

    assert!(
        checked >= PAIRINGS_FLOOR,
        "checked {checked} edge/guard-crate pairing(s) — the walk is reading the wrong tree",
    );
    assert!(
        offenders.is_empty(),
        "every `Guard::check_*` entry defaults to `Ok(())`, so an arm the \
         umbrella leaves compiled out is a guard that passes rather than one \
         that is absent: {offenders:#?}",
    );
}

/// Whether the file carries an `impl` block declaring a method of this name —
/// the shape that *answers* an edge, as opposed to the trait that declares it.
fn file_declares_impl_fn(file: &syn::File, method: &str) -> bool {
    file.items.iter().any(|item| {
        matches!(item, Item::Impl(block) if block.items.iter().any(|sub| {
            matches!(sub, syn::ImplItem::Fn(f) if f.sig.ident == method)
        }))
    })
}
