//! The transports join: every transport the framework ships, against the one
//! test that sums the way down.
//!
//! `App::run` awaits every transport's `serve` before the shutdown hooks and
//! holds no bound of its own, so each transport owes one, and the way down is
//! the longest of them, then the hooks' budget, then the flush. That sum is
//! pinned in `nest-rs-testing`'s `shutdown.rs`, one row per transport — and a
//! transport with no row is a stop nobody summed: the Redis worker drained for
//! 30 seconds under a 30-second grace period, and the sum read only HTTP's
//! window. Here the rows are held to the source.
//!
//! **Members are derived, never listed**: every `impl Transport for X` in a
//! framework crate's `src/`, outside `#[cfg(test)]`. **Coverage is read off the
//! test**: the string literals of its `transports()` function, which name each
//! row's transport. A transport with no row fails, and so does a row naming no
//! transport — a stale one would sum a stop nothing performs.
//!
//! **What it reads by its spelling**: the `Transport` trait where it is
//! implemented. The `blinds` join refuses a rename of it.

use std::collections::BTreeSet;
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::sources::{each_source, is_cfg_test, parsed, repo_root, string_literals};
use quote::ToTokens as _;
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![Followed::implemented(TRAIT)]
}

/// The trait a transport implements.
const TRAIT: &str = "Transport";

/// The test that sums the way down, and the function in it that lists one row
/// per transport.
const SUM: &str = "crates/nest-rs-testing/tests/integration/shutdown.rs";
const ROWS: &str = "transports";

/// Below this the walk is reading the wrong tree: HTTP, the Redis worker and
/// the scheduler are three.
const FLOOR: usize = 3;

/// The type of every `impl Transport for …` in `ast`, outside `#[cfg(test)]`.
fn implemented(ast: &syn::File) -> BTreeSet<String> {
    #[derive(Default)]
    struct Impls(BTreeSet<String>);

    impl<'ast> Visit<'ast> for Impls {
        fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
            if !is_cfg_test(&node.attrs) {
                syn::visit::visit_item_mod(self, node);
            }
        }

        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            if is_cfg_test(&node.attrs) {
                return;
            }
            let implements = node
                .trait_
                .as_ref()
                .and_then(|(path, _)| path.segments.last())
                .is_some_and(|segment| segment.ident == TRAIT);
            if implements
                && let syn::Type::Path(ty) = &*node.self_ty
                && let Some(segment) = ty.path.segments.last()
            {
                self.0.insert(segment.ident.to_string());
            }
        }
    }

    let mut impls = Impls::default();
    impls.visit_file(ast);
    impls.0
}

/// The transports the sum's rows name: the string literals of `transports()`.
fn rows(ast: &syn::File) -> BTreeSet<String> {
    ast.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == ROWS => {
                Some(string_literals(function.block.to_token_stream()))
            }
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Every transport a framework crate ships.
fn shipped(root: &Path) -> BTreeSet<String> {
    let mut transports = BTreeSet::new();
    each_source(root, |rel, ast| {
        if rel.starts_with("crates/") && rel.contains("/src/") {
            transports.extend(implemented(ast));
        }
    });
    transports
}

#[test]
fn every_transport_the_framework_ships_is_summed_on_the_way_down() {
    let root = repo_root();
    let shipped = shipped(&root);
    assert!(
        shipped.len() >= FLOOR,
        "found {} `impl Transport` — the walk is reading the wrong tree: {shipped:?}",
        shipped.len(),
    );
    let sum = parsed(&root.join(SUM)).unwrap_or_else(|| panic!("{SUM} parses"));
    let rows = rows(&sum);

    let unsummed: Vec<&String> = shipped.difference(&rows).collect();
    assert!(
        unsummed.is_empty(),
        "{unsummed:?} {} a transport with no row in {SUM}'s `{ROWS}()`: its stop is part of \
         the way down, and the sum that has to fit the grace period never reads it — add the \
         row, with the bound its `serve` keeps",
        if unsummed.len() == 1 { "is" } else { "are" },
    );
    let stale: Vec<&String> = rows.difference(&shipped).collect();
    assert!(
        stale.is_empty(),
        "{SUM}'s `{ROWS}()` sums {stale:?}, which no framework crate implements `Transport` \
         for — a row for a stop nothing performs; remove it",
    );
}

/// The reading itself, on a source whose answer is written beside it: a
/// transport is found by the trait's last segment however its path is spelled,
/// and a fixture behind `#[cfg(test)]` — an impl or a whole module — is not
/// shipped.
#[test]
fn a_transport_is_read_off_its_impl_and_a_fixture_is_not() {
    let ast: syn::File = syn::parse_str(
        r#"
        impl Transport for Bare {}
        impl nest_rs_core::Transport for Pathed {}
        impl Other for NotOne {}
        #[cfg(test)]
        impl Transport for Fixture {}
        #[cfg(test)]
        mod tests {
            impl Transport for InATest {}
        }
        fn transports() -> [(&'static str, ()); 2] {
            [("Bare", ()), ("Pathed", ())]
        }
        "#,
    )
    .expect("the planted source parses");

    assert_eq!(
        implemented(&ast),
        BTreeSet::from(["Bare".to_owned(), "Pathed".to_owned()]),
    );
    assert_eq!(
        rows(&ast),
        BTreeSet::from(["Bare".to_owned(), "Pathed".to_owned()]),
    );
}
