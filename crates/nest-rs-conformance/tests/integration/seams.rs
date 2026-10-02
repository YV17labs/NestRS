//! The seams join: every `for_root` a framework crate offers, against a boot
//! that runs it **in that crate's own suite**.
//!
//! `CLAUDE.md`'s *Shipping a new capability* names two witnesses, and the second
//! is this one: "a test in the capability's **own crate** that boots the
//! documented wiring […] Every `for_root` seam has one; that is the bar."
//! Nothing checked it, and the seam a nobody-boots is not a theoretical hole —
//! a `for_root` is the one path a consumer has to pin a module's config from
//! code, so an unbooted one is a documented entry point with no evidence it
//! resolves.
//!
//! **This is the one join that is deliberately not workspace-wide**, and the
//! asymmetry is the rule's, not a shortcut: the obligation *is* locality. A
//! `demo/` app booting `WsModule::for_root` proves the product wires it; it does
//! not give `nest-rs-ws` the composition witness its own release owes. Every
//! other family here takes the workspace-wide reading.
//!
//! Members come from the `impl` blocks themselves, and the key is how the seam
//! is called — `<Module>::for_root`, the same four tokens in a test body as in
//! an `imports = [..]` list.
//!
//! **What it reads by its spelling**: `pub fn for_root` in an inherent `impl`,
//! at any depth outside `#[cfg(test)]`. One a `macro_rules!` writes is refused
//! by the `blinds` join.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    executed_tokens, is_cfg_test, parsed, relative, repo_root, rust_files, spells_path,
};
use proc_macro2::TokenStream;
use syn::{ImplItem, Type, Visibility};

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![Followed::declaration("for_root")]
}

const BASELINE: &str = "seams-baseline.txt";

/// Fourteen seams stand today, one per configurable module plus `ConfigModule`'s
/// homonym. Below that the scan is reading the wrong tree and every hole it
/// reports is an artefact.
const FLOOR: usize = 14;

/// A seam as its own `impl` block spells it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Seam {
    /// The crate directory that owns it — the only crate whose suite counts.
    krate: String,
    /// The module type the seam hangs off, e.g. `WsModule`.
    module: String,
}

impl Seam {
    fn key(&self) -> String {
        format!("{}::for_root :: {}", self.module, self.krate)
    }
}

/// The bare type name an inherent `impl` is written for, or `None` for a shape
/// no seam is ever declared on (a reference, a tuple, a trait impl).
fn inherent_self_ty(item: &syn::ItemImpl) -> Option<String> {
    if item.trait_.is_some() {
        return None;
    }
    let Type::Path(path) = &*item.self_ty else {
        return None;
    };
    Some(path.path.segments.last()?.ident.to_string())
}

fn declares_for_root(item: &syn::ItemImpl) -> bool {
    item.items.iter().any(|member| match member {
        ImplItem::Fn(f) => matches!(f.vis, Visibility::Public(_)) && f.sig.ident == "for_root",
        _ => false,
    })
}

/// Every `pub fn for_root` a framework crate declares.
///
/// `cli/src/templates/` is excluded for the reason the events join excludes it:
/// that tree is the *generated project's* source, so a seam spelled there
/// belongs to a scaffolded app's family and is proved by the CLI's own scaffold
/// suite.
fn declared_seams() -> Vec<Seam> {
    let root = repo_root();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let krate = entry.path();
        let Some(name) = krate.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for file in rust_files(&krate.join("src")) {
            if relative(&file, &root).contains("cli/src/templates/") {
                continue;
            }
            let Some(ast) = parsed(&file) else {
                continue;
            };
            out.extend(seams_in(&ast).into_iter().map(|module| Seam {
                krate: name.to_owned(),
                module,
            }));
        }
    }
    out
}

/// The module types a file's inherent `impl`s hang a `pub fn for_root` off, at
/// any depth outside `#[cfg(test)]` — top-level `impl`s alone left one inside an
/// inline `mod` unjoined.
fn seams_in(file: &syn::File) -> Vec<String> {
    struct Impls(Vec<String>);
    impl<'ast> syn::visit::Visit<'ast> for Impls {
        fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
            if !is_cfg_test(&node.attrs) {
                syn::visit::visit_item_mod(self, node);
            }
        }
        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            if !is_cfg_test(&node.attrs) {
                syn::visit::visit_item_fn(self, node);
            }
        }
        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            if is_cfg_test(&node.attrs) {
                return;
            }
            if declares_for_root(node)
                && let Some(module) = inherent_self_ty(node)
            {
                self.0.push(module);
            }
            syn::visit::visit_item_impl(self, node);
        }
    }
    let mut impls = Impls(Vec::new());
    syn::visit::Visit::visit_file(&mut impls, file);
    impls.0
}

/// A seam one `mod` down is a seam; a test's is not.
#[test]
fn a_seam_is_read_at_any_depth() {
    let file: syn::File = syn::parse_quote! {
        impl TopModule { pub fn for_root() {} }
        mod inner { impl InnerModule { pub fn for_root() {} } }
        #[cfg(test)]
        mod tests { impl FixtureModule { pub fn for_root() {} } }
        impl Private { fn for_root() {} }
    };
    assert_eq!(seams_in(&file), ["TopModule", "InnerModule"]);
}

/// The seams no crate's own suite executes.
///
/// Grouped by crate, because the corpus a seam is looked for in is its *crate's*
/// and not its own: three crates carry more than one `for_root`, so asking per
/// seam re-read and re-parsed the same `src` + `tests` tree once per sibling.
fn unbooted(seams: &[Seam], root: &Path) -> BTreeSet<String> {
    let mut by_crate: BTreeMap<&str, Vec<&Seam>> = BTreeMap::new();
    for seam in seams {
        by_crate.entry(seam.krate.as_str()).or_default().push(seam);
    }
    let mut out = BTreeSet::new();
    for (krate, seams) in by_crate {
        let dir = root.join("crates").join(krate);
        let mut sources = rust_files(&dir.join("tests"));
        sources.extend(rust_files(&dir.join("src")));
        let corpus: Vec<TokenStream> = sources
            .iter()
            .flat_map(|path| executed_tokens(path, root))
            .collect();
        out.extend(
            seams
                .into_iter()
                .filter(|seam| {
                    !corpus
                        .iter()
                        .any(|tokens| spells_path(tokens, &seam.module, "for_root"))
                })
                .map(Seam::key),
        );
    }
    out
}

#[test]
fn every_for_root_seam_is_booted_by_its_own_crate() {
    let root = repo_root();
    let seams = declared_seams();
    baseline::floor(seams.len(), FLOOR, "`pub fn for_root` seam(s)");

    let holes = unbooted(&seams, &root);

    baseline::gate(
        BASELINE,
        &holes,
        seams.len(),
        "seams",
        "`for_root` seam(s)",
        "a `for_root` its own crate never boots",
    );
}
