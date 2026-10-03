//! Print the canon: every fact about the framework a docs page is checked
//! against, as JSON on stdout.
//!
//! **A generator, never a committed file.** `docs/scripts/lint-docs.mjs` runs
//! this binary each time it starts and reads what it prints, so the facts are as
//! old as the tree and no older. The canon used to be a test that rewrote
//! `docs/canon.json` and failed until the file was committed: half of that
//! file's commits only bumped the test count, and parallel branches collided on
//! it. Nothing is written now, so nothing can be stale.
//!
//! **One derivation, in Rust.** The linter reads these facts and never
//! re-derives them; two readers of one definition is how it once counted 27
//! capabilities against a landing that correctly said 28. Raw demo sources do
//! not travel here — the linter reads `demo/` itself, because file content is
//! not a derivation and cannot drift.
//!
//! **What travels, and in which of two shapes.** A fact whose comparison needs
//! no grammar travels **extracted** (a list of names, checked with `includes`);
//! a fact the linter compares by applying one extractor to both sides travels
//! **raw**, so that extractor stays single and on the linter's side.
//! `architecture` is the only member of the second kind.
//!
//! **Floors.** Each population has one, so a walk reading the wrong tree fails
//! here instead of publishing an empty canon every page then agrees with.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;
use std::path::Path;

use nest_rs_conformance::sources::{
    config_namespace, crate_dirs, declared_targets, declared_units, exported_decorators,
    is_cfg_test, parsed, read, relative, repo_root, rust_files, umbrella_matrix,
};
use serde::Serialize;
use syn::visit::Visit;
use syn::{ItemFn, ItemTrait, TraitItem};

/// Why the canon could not be derived. `Debug` is what `main` prints on `Err`,
/// so it renders the sentence and nothing else.
struct Refusal(String);

impl fmt::Debug for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<E: std::error::Error> From<E> for Refusal {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

fn refuse<T>(why: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(why.into()))
}

/// Below its floor a population is being read from the wrong tree.
fn floor(count: usize, floor: usize, what: &str) -> Result<(), Refusal> {
    if count < floor {
        return refuse(format!(
            "read {count} {what}, below the floor of {floor} — the walk is reading the wrong \
             tree, and an empty canon is one every page agrees with"
        ));
    }
    Ok(())
}

mod floors {
    pub(super) const CAPABILITIES: usize = 20;
    pub(super) const DECORATORS: usize = 20;
    pub(super) const LAYER_SUBTRAITS: usize = 4;
    pub(super) const TRAITS: usize = 50;
    pub(super) const TESTS: usize = 1_500;
    pub(super) const CONFIGS: usize = 12;
    pub(super) const ENVELOPE_KEYS: usize = 4;
    pub(super) const ROLE_ROWS: usize = 20;
    pub(super) const UNITS: usize = 8;
    pub(super) const TARGETS: usize = 20;
    pub(super) const QUEUE_CAPABILITIES: usize = 4;
}

/// A `#[config]` struct, as the page publishing its key table has to describe
/// it.
#[derive(Serialize)]
struct ConfigFacts {
    /// The repo-relative file that declares it, for a message a reader can open.
    source: String,
    /// Field names in declaration order — the keys a table claiming to be
    /// exhaustive owes a row each.
    fields: Vec<String>,
    /// Whether `defaults()` branches on the profile: a page publishing the dev
    /// branch as *the* default is wrong for production.
    profile_split: bool,
}

/// The two regions of the architecture rules a page restates, sliced and not
/// extracted.
#[derive(Serialize)]
struct ArchitectureFacts {
    /// Every table row of the rules file, verbatim.
    role_rows: Vec<String>,
    /// The fenced block listing the words a module may not be named after.
    reserved_block: String,
}

/// Everything a docs page can be checked against.
#[derive(Serialize)]
struct Canon {
    /// Every capability a developer can name in `--features`.
    capabilities: Vec<String>,
    /// Every crate a capability activates, mapped to that capability — what a
    /// crate's README owes an install line for.
    capability_crates: BTreeMap<String, String>,
    /// Every `#[proc_macro_attribute]` the framework exports.
    decorators: Vec<String>,
    /// Every `pub trait X: Layer` — the Layer System's members.
    layer_subtraits: Vec<String>,
    /// Every `pub trait` the framework ships, mapped to the methods it declares.
    trait_methods: BTreeMap<String, Vec<String>>,
    /// The landing's `N+ tests` floor.
    test_count: usize,
    /// `major.minor` of the framework the repo builds.
    version_req: String,
    /// The binding `nest-rs-opentelemetry`'s boot panic tells a reader to write.
    otel_binding: String,
    /// The keys the port seals into a `nest_rs_queue::Envelope` on the wire.
    envelope_keys: Vec<String>,
    /// Every `#[config]` struct, keyed by type name.
    configs: BTreeMap<String, ConfigFacts>,
    architecture: ArchitectureFacts,
    /// Every unit of work an edge declares — `<edge>.<unit>`.
    units: Vec<String>,
    /// Every span target a framework crate declares.
    targets: Vec<String>,
    /// Every variant of `nest_rs_queue::Capability`.
    queue_capabilities: Vec<String>,
}

fn main() -> Result<(), Refusal> {
    let canon = derive(&repo_root())?;
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &canon)?;
    writeln!(out)?;
    Ok(())
}

/// Read every fact, with a floor on each population.
fn derive(root: &Path) -> Result<Canon, Refusal> {
    let matrix = umbrella_matrix(root);
    let capabilities = matrix.features();
    floor(
        capabilities.len(),
        floors::CAPABILITIES,
        "umbrella capabilities",
    )?;
    let capability_crates = matrix
        .crates()
        .into_iter()
        .map(|(feature, krate)| (krate, feature))
        .collect();

    let rust = rust_facts(root);
    floor(rust.decorators.len(), floors::DECORATORS, "decorators")?;
    floor(
        rust.layer_subtraits.len(),
        floors::LAYER_SUBTRAITS,
        "`pub trait _: Layer` declarations",
    )?;
    floor(rust.trait_methods.len(), floors::TRAITS, "`pub trait`s")?;
    floor(rust.tests, floors::TESTS, "test functions")?;
    floor(rust.configs.len(), floors::CONFIGS, "`#[config]` structs")?;

    let envelope_keys = envelope_keys(root)?;
    floor(
        envelope_keys.len(),
        floors::ENVELOPE_KEYS,
        "queue envelope keys",
    )?;

    let architecture = architecture(root)?;
    floor(
        architecture.role_rows.len(),
        floors::ROLE_ROWS,
        "architecture table rows",
    )?;
    if architecture.reserved_block.is_empty() {
        return refuse(
            "no reserved-vocabulary block in the architecture rules — teach `architecture` \
             where it moved, do not publish an empty one",
        );
    }

    let units: BTreeSet<String> = declared_units().into_iter().map(|(u, _, _)| u).collect();
    floor(units.len(), floors::UNITS, "units of work")?;
    let targets: BTreeSet<String> = declared_targets()
        .iter()
        .map(|(target, _, _)| (*target).to_owned())
        .collect();
    floor(targets.len(), floors::TARGETS, "span targets")?;
    let queue_capabilities = queue_capabilities(root)?;
    floor(
        queue_capabilities.len(),
        floors::QUEUE_CAPABILITIES,
        "`nest_rs_queue::Capability` variants",
    )?;

    Ok(Canon {
        capabilities,
        capability_crates,
        decorators: rust.decorators,
        layer_subtraits: rust.layer_subtraits,
        trait_methods: rust.trait_methods,
        test_count: rust.tests,
        version_req: version_req(root)?,
        otel_binding: otel_binding(root)?,
        envelope_keys,
        configs: rust.configs,
        architecture,
        units: units.into_iter().collect(),
        targets: targets.into_iter().collect(),
        queue_capabilities,
    })
}

/// What one walk of `crates/` yields.
#[derive(Default)]
struct RustFacts {
    decorators: Vec<String>,
    layer_subtraits: Vec<String>,
    trait_methods: BTreeMap<String, Vec<String>>,
    tests: usize,
    configs: BTreeMap<String, ConfigFacts>,
}

fn rust_facts(root: &Path) -> RustFacts {
    let mut out = RustFacts::default();
    let mut decorators = BTreeSet::new();
    let mut subtraits = BTreeSet::new();
    let mut methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for dir in crate_dirs() {
        if dir.starts_with(root.join("crates")) {
            decorators.extend(exported_decorators(&dir));
        }
    }

    for file in rust_files(&root.join("crates")) {
        let Some(ast) = parsed(&file) else {
            continue;
        };
        let rel = relative(&file, root);
        let mut scan = FileScan::default();
        scan.visit_file(&ast);
        out.tests += scan.tests;

        // Declarations come from what ships; the test count from everywhere. A
        // suite fixture declaring a `#[config]` is not a framework config.
        if rel.contains("/tests/") {
            continue;
        }
        subtraits.extend(scan.layer_subtraits);
        for (name, fns) in scan.trait_methods {
            methods.entry(name).or_default().extend(fns);
        }
        for config in scan.configs {
            out.configs.insert(
                config.name,
                ConfigFacts {
                    source: rel.clone(),
                    fields: config.fields,
                    profile_split: config.profile_split,
                },
            );
        }
    }

    out.decorators = decorators.into_iter().collect();
    out.layer_subtraits = subtraits.into_iter().collect();
    out.trait_methods = methods
        .into_iter()
        .map(|(name, fns)| (name, fns.into_iter().collect()))
        .collect();
    out
}

/// A `#[config]` struct as one file declares it, before its source is known.
struct ConfigDecl {
    name: String,
    fields: Vec<String>,
    profile_split: bool,
}

/// One file's contribution, gathered in a single `syn` walk. A test is any
/// function carrying an attribute whose last path segment is `test`.
#[derive(Default)]
struct FileScan {
    tests: usize,
    layer_subtraits: Vec<String>,
    trait_methods: Vec<(String, Vec<String>)>,
    configs: Vec<ConfigDecl>,
    /// Depth inside a `#[cfg(test)]` module, whose declarations are fixtures.
    under_cfg_test: usize,
}

fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "test")
    })
}

impl<'ast> Visit<'ast> for FileScan {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let fixture = usize::from(is_cfg_test(&node.attrs));
        self.under_cfg_test += fixture;
        syn::visit::visit_item_mod(self, node);
        self.under_cfg_test -= fixture;
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.tests += usize::from(is_test(&node.attrs));
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.tests += usize::from(is_test(&node.attrs));
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        if self.under_cfg_test == 0 && matches!(node.vis, syn::Visibility::Public(_)) {
            let name = node.ident.to_string();
            if node.supertraits.iter().any(|b| bound_is(b, "Layer")) {
                self.layer_subtraits.push(name.clone());
            }
            let fns = node
                .items
                .iter()
                .filter_map(|item| match item {
                    TraitItem::Fn(f) => Some(f.sig.ident.to_string()),
                    _ => None,
                })
                .collect();
            self.trait_methods.push((name, fns));
        }
        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.under_cfg_test == 0 && config_namespace(&node.attrs).is_some() {
            self.configs.push(ConfigDecl {
                name: node.ident.to_string(),
                fields: node
                    .fields
                    .iter()
                    .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
                    .collect(),
                profile_split: false,
            });
        }
        syn::visit::visit_item_struct(self, node);
    }

    /// `fn defaults()` lives in the `Config` impl beside the struct, so the two
    /// halves are stitched here rather than in a second pass.
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let branches = node.items.iter().any(|item| {
            let syn::ImplItem::Fn(f) = item else {
                return false;
            };
            f.sig.ident == "defaults" && spells_dev_profile(f)
        });
        if branches
            && let syn::Type::Path(p) = &*node.self_ty
            && let Some(last) = p.path.segments.last()
            && let Some(decl) = self.configs.iter_mut().find(|c| last.ident == c.name)
        {
            decl.profile_split = true;
        }
        syn::visit::visit_item_impl(self, node);
    }
}

/// Whether a supertrait bound names `name` as its last segment.
fn bound_is(bound: &syn::TypeParamBound, name: &str) -> bool {
    let syn::TypeParamBound::Trait(t) = bound else {
        return false;
    };
    t.path.segments.last().is_some_and(|s| s.ident == name)
}

/// Whether a `defaults()` body consults the profile.
fn spells_dev_profile(f: &syn::ImplItemFn) -> bool {
    use quote::ToTokens;
    f.block
        .to_token_stream()
        .to_string()
        .contains("dev_profile")
}

/// `major.minor` of the framework, from the root manifest's
/// `[workspace.package]`.
fn version_req(root: &Path) -> Result<String, Refusal> {
    let doc: toml_edit::DocumentMut = read(&root.join("Cargo.toml"))?.parse()?;
    let Some(version) = doc
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
    else {
        return refuse("the root manifest declares no `[workspace.package] version`");
    };
    let mut parts = version.split('.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => Ok(format!("{major}.{minor}")),
        _ => refuse(format!("`{version}` is not `major.minor.patch`")),
    }
}

/// The binding `nest-rs-opentelemetry`'s boot panic names — read out of the
/// panic rather than restated, so the page and the message cannot disagree.
fn otel_binding(root: &Path) -> Result<String, Refusal> {
    const MARKER: &str = "Add `let ";
    let text = read(&root.join("crates/nest-rs-opentelemetry/src/module.rs"))?;
    let binding = text.find(MARKER).and_then(|at| {
        text[at + MARKER.len()..]
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .find(|word| !word.is_empty())
    });
    match binding {
        Some(binding) => Ok(binding.to_owned()),
        None => refuse(
            "nest-rs-opentelemetry's boot panic no longer reads \"Add `let <binding> =\" — \
             teach `otel_binding` the new wording, do not delete the fact",
        ),
    }
}

/// The string constants `nest_rs_queue::envelope` declares: the keys a
/// third-party driver has to carry across the process hop.
fn envelope_keys(root: &Path) -> Result<Vec<String>, Refusal> {
    let Some(ast) = parsed(&root.join("crates/nest-rs-queue/src/envelope.rs")) else {
        return refuse("crates/nest-rs-queue/src/envelope.rs does not parse");
    };
    let mut keys = BTreeSet::new();
    for item in &ast.items {
        if let syn::Item::Const(c) = item
            && let syn::Expr::Lit(lit) = &*c.expr
            && let syn::Lit::Str(s) = &lit.lit
        {
            keys.insert(s.value());
        }
    }
    Ok(keys.into_iter().collect())
}

/// The variants of `nest_rs_queue::Capability`, in declaration order.
fn queue_capabilities(root: &Path) -> Result<Vec<String>, Refusal> {
    let Some(ast) = parsed(&root.join("crates/nest-rs-queue/src/capability.rs")) else {
        return refuse("crates/nest-rs-queue/src/capability.rs does not parse");
    };
    Ok(ast
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Enum(e) if e.ident == "Capability" => Some(e),
            _ => None,
        })
        .flat_map(|e| e.variants.iter().map(|v| v.ident.to_string()))
        .collect())
}

/// The two regions of the architecture rules the `/architecture/` page restates:
/// the table rows, and the fenced reserved-vocabulary block. Read from the
/// template the CLI ships into every scaffolded project.
fn architecture(root: &Path) -> Result<ArchitectureFacts, Refusal> {
    const RESERVED_SECTIONS: [&str; 2] = ["Reserved vocabulary", "What a folder may not be called"];

    let text = read(&root.join("crates/nest-rs-cli/src/templates/architecture.md"))?;
    let role_rows = text
        .lines()
        .filter(|line| line.starts_with('|'))
        .map(str::to_owned)
        .collect();

    let mut reserved_block = String::new();
    let mut section = "";
    let mut inside = false;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.trim();
            continue;
        }
        if line.starts_with("```") {
            if inside {
                break;
            }
            inside = RESERVED_SECTIONS.contains(&section);
            continue;
        }
        if inside {
            reserved_block.push_str(line);
            reserved_block.push('\n');
        }
    }

    Ok(ArchitectureFacts {
        role_rows,
        reserved_block,
    })
}
