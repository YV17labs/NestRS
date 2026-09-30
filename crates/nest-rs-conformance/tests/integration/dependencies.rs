//! The dependencies join: every site a framework crate names an **optional**
//! dependency, against the `#[cfg(feature = …)]` gates above it.
//!
//! A crate compiles alone only if every path it spells resolves under the
//! features it was asked for. An optional dependency exists only while a feature
//! enabling it is on, so a path rooted at one must sit below a gate whose
//! feature enables it — at any depth: the `mod` that holds the file, the item,
//! the statement, or a macro call's tokens.
//!
//! That shipped. `nest-rs-authz`'s engine wrote `nest_rs_core::error_message`
//! inside a `tracing::warn!` while `nest-rs-core` was reached only by the
//! transport features, so the crate did not compile under its own defaults —
//! and neither did the umbrella's `authz` or any headless `seaorm` build. No
//! suite could see it: `--workspace` unifies every member's features into one
//! build, the hygiene witness enables all of them at once, and CI is not the
//! gate (`manifests-ci.md`). This join is the gate the Definition of done does
//! run, and it reads what a build cannot be asked cheaply: every crate under
//! every feature, not only the union.
//!
//! What it cannot see is a *forwarded* feature — a path that resolves in a
//! dependency only while that dependency's own feature is on. That half needs a
//! real build per crate and per umbrella feature, and runs as a monitor
//! (`security-watch.yml`, `feature-matrix`).
//!
//! Members are derived: every crate under `crates/`, its `[dependencies]`, its
//! `[features]`, and the `mod` tree from `src/lib.rs` or `src/main.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{is_cfg_test, parsed, path_roots, relative, repo_root};
use proc_macro2::TokenStream;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, Expr, Item, Meta, Token, UseTree};

/// Below this the walk is reading the wrong tree.
const FLOOR: usize = 200;

/// One crate's dependency surface: which dependencies are optional, and which
/// optional dependencies each feature turns on, transitively.
#[derive(Default)]
struct Manifest {
    optional: BTreeSet<String>,
    enables: BTreeMap<String, BTreeSet<String>>,
}

impl Manifest {
    /// Read `Cargo.toml`, or `None` when it is not one this join can read.
    fn read(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
        let mut manifest = Self::default();
        if let Some(deps) = doc.get("dependencies").and_then(|d| d.as_table_like()) {
            for (key, value) in deps.iter() {
                let optional = value
                    .as_table_like()
                    .and_then(|t| t.get("optional"))
                    .and_then(|o| o.as_bool())
                    .unwrap_or(false);
                if optional {
                    manifest.optional.insert(key.replace('-', "_"));
                }
            }
        }
        let mut direct: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(features) = doc.get("features").and_then(|f| f.as_table_like()) {
            for (feature, value) in features.iter() {
                let entries = value
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|e| e.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                direct.insert(feature.to_owned(), entries);
            }
        }
        // Cargo's implicit feature: an optional dependency no `dep:` entry
        // names is a feature of its own name, enabling itself.
        let named: BTreeSet<String> = direct
            .values()
            .flatten()
            .filter_map(|e| e.strip_prefix("dep:"))
            .map(|d| d.replace('-', "_"))
            .collect();
        for dep in &manifest.optional {
            if !named.contains(dep) {
                direct
                    .entry(dep.clone())
                    .or_default()
                    .push(format!("dep:{dep}"));
            }
        }
        for feature in direct.keys() {
            let mut seen = BTreeSet::new();
            let mut out = BTreeSet::new();
            closure(feature, &direct, &manifest.optional, &mut seen, &mut out);
            manifest.enables.insert(feature.replace('-', "_"), out);
        }
        Some(manifest)
    }

    /// Whether the cfg predicate `pred` can only hold while `dep` is enabled.
    fn requires(&self, pred: &Meta, dep: &str) -> bool {
        match pred {
            Meta::NameValue(nv) if nv.path.is_ident("feature") => match &nv.value {
                Expr::Lit(lit) => match &lit.lit {
                    syn::Lit::Str(s) => self
                        .enables
                        .get(&s.value().replace('-', "_"))
                        .is_some_and(|deps| deps.contains(dep)),
                    _ => false,
                },
                _ => false,
            },
            Meta::List(list) if list.path.is_ident("all") => nested(list.tokens.clone())
                .iter()
                .any(|p| self.requires(p, dep)),
            Meta::List(list) if list.path.is_ident("any") => {
                let alternatives = nested(list.tokens.clone());
                !alternatives.is_empty() && alternatives.iter().all(|p| self.requires(p, dep))
            }
            _ => false,
        }
    }
}

/// What `feature` turns on, following feature-to-feature entries. `dep:x` and
/// `x/f` enable `x`; `x?/f` enables nothing on its own.
fn closure(
    feature: &str,
    direct: &BTreeMap<String, Vec<String>>,
    optional: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    if !seen.insert(feature.to_owned()) {
        return;
    }
    for entry in direct.get(feature).into_iter().flatten() {
        if let Some(dep) = entry.strip_prefix("dep:") {
            out.insert(dep.replace('-', "_"));
        } else if let Some((dep, _)) = entry.split_once('/') {
            if !dep.ends_with('?') && optional.contains(&dep.replace('-', "_")) {
                out.insert(dep.replace('-', "_"));
            }
        } else if direct.contains_key(entry) {
            closure(entry, direct, optional, seen, out);
        }
    }
}

fn nested(tokens: TokenStream) -> Vec<Meta> {
    syn::parse::Parser::parse2(Punctuated::<Meta, Token![,]>::parse_terminated, tokens)
        .map(|list| list.into_iter().collect())
        .unwrap_or_default()
}

/// The `cfg` predicates among `attrs`.
fn cfgs(attrs: &[Attribute]) -> Vec<Meta> {
    attrs
        .iter()
        .filter_map(|attr| match &attr.meta {
            Meta::List(list) if list.path.is_ident("cfg") => {
                syn::parse2::<Meta>(list.tokens.clone()).ok()
            }
            _ => None,
        })
        .collect()
}

/// A walk over one crate's `mod` tree, carrying the gates above each site.
struct Walk<'m> {
    manifest: &'m Manifest,
    file: String,
    gates: Vec<Meta>,
    holes: BTreeSet<String>,
    /// `mod x;` declarations to read next, with the directory their file
    /// resolves against and the gates in force at the declaration.
    pending: Vec<(PathBuf, String, Vec<Meta>)>,
    dir: PathBuf,
}

impl Walk<'_> {
    fn check(&mut self, root: &str) {
        if self.manifest.optional.contains(root)
            && !self.gates.iter().any(|g| self.manifest.requires(g, root))
        {
            self.holes.insert(format!(
                "{} :: `{root}` is optional and no feature gating this site enables it",
                self.file,
            ));
        }
    }

    fn check_tokens(&mut self, tokens: &TokenStream) {
        for root in path_roots(tokens) {
            self.check(&root);
        }
    }

    /// Run `visit` with `attrs`' cfg predicates pushed — or not at all under
    /// `#[cfg(test)]`, which compiles with the dev-dependencies.
    fn gated(&mut self, attrs: &[Attribute], visit: impl FnOnce(&mut Self)) {
        if is_cfg_test(attrs) {
            return;
        }
        let pushed = cfgs(attrs);
        let depth = self.gates.len();
        self.gates.extend(pushed);
        visit(self);
        self.gates.truncate(depth);
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// The attributes of the expression shapes a `#[cfg]` sits on in statement
/// position. A shape missing here reads as ungated, which reports rather than
/// hides.
fn expr_attrs(expr: &Expr) -> &[Attribute] {
    match expr {
        Expr::Assign(e) => &e.attrs,
        Expr::Async(e) => &e.attrs,
        Expr::Await(e) => &e.attrs,
        Expr::Block(e) => &e.attrs,
        Expr::Call(e) => &e.attrs,
        Expr::Closure(e) => &e.attrs,
        Expr::ForLoop(e) => &e.attrs,
        Expr::If(e) => &e.attrs,
        Expr::Let(e) => &e.attrs,
        Expr::Loop(e) => &e.attrs,
        Expr::Macro(e) => &e.attrs,
        Expr::Match(e) => &e.attrs,
        Expr::MethodCall(e) => &e.attrs,
        Expr::Path(e) => &e.attrs,
        Expr::Return(e) => &e.attrs,
        Expr::Try(e) => &e.attrs,
        Expr::Unsafe(e) => &e.attrs,
        Expr::While(e) => &e.attrs,
        _ => &[],
    }
}

fn use_roots(tree: &UseTree, out: &mut Vec<String>) {
    match tree {
        UseTree::Path(p) => out.push(p.ident.to_string()),
        UseTree::Name(n) => out.push(n.ident.to_string()),
        UseTree::Rename(r) => out.push(r.ident.to_string()),
        UseTree::Group(g) => g.items.iter().for_each(|t| use_roots(t, out)),
        UseTree::Glob(_) => {}
    }
}

impl<'ast> Visit<'ast> for Walk<'_> {
    fn visit_item(&mut self, node: &'ast Item) {
        self.gated(item_attrs(node), |walk| match node {
            Item::Mod(m) if m.content.is_none() => {
                walk.pending
                    .push((walk.dir.clone(), m.ident.to_string(), walk.gates.clone()));
                for attr in &m.attrs {
                    walk.visit_attribute(attr);
                }
            }
            Item::Mod(m) => {
                let outer = walk.dir.clone();
                walk.dir = outer.join(m.ident.to_string());
                syn::visit::visit_item_mod(walk, m);
                walk.dir = outer;
            }
            Item::Use(u) => {
                let mut roots = Vec::new();
                // `use ::x::…` and `use x::…` both root at `x`.
                use_roots(&u.tree, &mut roots);
                for root in roots {
                    walk.check(&root);
                }
            }
            _ => syn::visit::visit_item(walk, node),
        });
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        let attrs = match node {
            syn::ImplItem::Const(i) => &i.attrs[..],
            syn::ImplItem::Fn(i) => &i.attrs,
            syn::ImplItem::Type(i) => &i.attrs,
            syn::ImplItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        self.gated(attrs, |walk| syn::visit::visit_impl_item(walk, node));
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        let attrs = match node {
            syn::TraitItem::Const(i) => &i.attrs[..],
            syn::TraitItem::Fn(i) => &i.attrs,
            syn::TraitItem::Type(i) => &i.attrs,
            syn::TraitItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        self.gated(attrs, |walk| syn::visit::visit_trait_item(walk, node));
    }

    fn visit_field(&mut self, node: &'ast syn::Field) {
        self.gated(&node.attrs, |walk| syn::visit::visit_field(walk, node));
    }

    fn visit_variant(&mut self, node: &'ast syn::Variant) {
        self.gated(&node.attrs, |walk| syn::visit::visit_variant(walk, node));
    }

    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        self.gated(&node.attrs, |walk| syn::visit::visit_arm(walk, node));
    }

    fn visit_field_value(&mut self, node: &'ast syn::FieldValue) {
        self.gated(&node.attrs, |walk| {
            syn::visit::visit_field_value(walk, node)
        });
    }

    fn visit_stmt(&mut self, node: &'ast syn::Stmt) {
        match node {
            syn::Stmt::Local(local) => {
                self.gated(&local.attrs, |walk| syn::visit::visit_stmt(walk, node));
            }
            syn::Stmt::Expr(expr, _) => {
                self.gated(expr_attrs(expr), |walk| syn::visit::visit_stmt(walk, node));
            }
            syn::Stmt::Macro(mac) => {
                self.gated(&mac.attrs, |walk| syn::visit::visit_stmt(walk, node));
            }
            // An item carries its own gate, read by `visit_item`.
            syn::Stmt::Item(_) => syn::visit::visit_stmt(self, node),
        }
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        if (node.segments.len() > 1 || node.leading_colon.is_some())
            && let Some(first) = node.segments.first()
        {
            self.check(&first.ident.to_string());
        }
        syn::visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.check_tokens(&node.tokens);
        syn::visit::visit_macro(self, node);
    }

    fn visit_attribute(&mut self, node: &'ast Attribute) {
        if let Meta::List(list) = &node.meta
            && !list.path.is_ident("cfg")
        {
            self.check_tokens(&list.tokens);
        }
        syn::visit::visit_attribute(self, node);
    }
}

/// The holes in one crate, and how many source files the walk read.
fn crate_holes(dir: &Path, root: &Path) -> (BTreeSet<String>, usize) {
    let Some(manifest) = Manifest::read(dir) else {
        return (BTreeSet::new(), 0);
    };
    let mut walk = Walk {
        manifest: &manifest,
        file: String::new(),
        gates: Vec::new(),
        holes: BTreeSet::new(),
        pending: Vec::new(),
        dir: PathBuf::new(),
    };
    let mut files = Vec::new();
    for entry in ["src/lib.rs", "src/main.rs"] {
        let path = dir.join(entry);
        if path.is_file() {
            files.push((path, Vec::new(), dir.join("src")));
        }
    }
    let mut read = 0;
    while let Some((path, gates, children)) = files.pop() {
        let Some(ast) = parsed(&path) else {
            continue;
        };
        read += 1;
        walk.file = relative(&path, root);
        walk.gates = gates;
        walk.dir = children;
        walk.visit_file(&ast);
        // A file `x.rs`'s children resolve under `x/`, and `lib.rs` / `mod.rs`'s
        // under their own folder: handing every child `parent/<name>` is both.
        for (parent, name, gates) in walk.pending.drain(..) {
            let flat = parent.join(format!("{name}.rs"));
            let nested = parent.join(&name).join("mod.rs");
            let child = if flat.is_file() { flat } else { nested };
            files.push((child, gates, parent.join(&name)));
        }
    }
    (walk.holes, read)
}

#[test]
fn every_optional_dependency_is_named_only_where_a_feature_enables_it() {
    let root = repo_root();
    let mut holes = BTreeSet::new();
    let mut read = 0;
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        panic!("the framework's crates/ is readable");
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let (found, files) = crate_holes(&dir, &root);
        holes.extend(found);
        read += files;
    }
    baseline::floor(read, FLOOR, "source file(s) in the framework's mod trees");
    assert!(
        holes.is_empty(),
        "{} site(s) name an optional dependency where no feature enabling it is \
         on, so the crate does not compile under the features that reach them. \
         Gate the site on a feature that enables the dependency, or make the \
         dependency non-optional:\n  {}",
        holes.len(),
        holes.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );
}

/// The join's own verdict on a tree whose answer is written beside it: the
/// ungated use in a macro call is the shape that shipped, and every gated
/// shape beside it must read as gated.
#[test]
fn the_join_sees_an_ungated_optional_path_at_every_depth() {
    const TREE: [(&str, &str); 4] = [
        (
            "Cargo.toml",
            r#"
[package]
name = "probe"
[features]
default = []
http = ["dep:opt-a", "glue"]
glue = ["opt-b/extra"]
weak = ["opt-c?/extra"]
[dependencies]
always = "1.0"
opt-a = { version = "1.0", optional = true }
opt-b = { version = "1.0", optional = true }
opt-c = { version = "1.0", optional = true }
opt-d = { version = "1.0", optional = true }
"#,
        ),
        (
            "src/lib.rs",
            r#"
#[cfg(feature = "http")]
mod http;
mod engine;
#[cfg(test)]
mod tests { fn t() { opt_d::x(); } }
"#,
        ),
        (
            "src/http.rs",
            "mod nested; fn a() { opt_a::x(); opt_b::y(); }",
        ),
        (
            "src/engine.rs",
            r#"
use always::Thing;
fn logs() { tracing::warn!(error = %opt_a::message()); }
#[cfg(any(feature = "http", feature = "glue"))]
fn either() { opt_b::y(); }
#[cfg(any(feature = "http", feature = "weak"))]
fn weakly() { opt_c::y(); }
fn body() {
    #[cfg(all(feature = "http", not(test)))]
    let _ = opt_a::x();
    #[cfg(feature = "opt-d")]
    { opt_d::x(); }
}
"#,
        ),
    ];
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dependencies-join");
    let _ = std::fs::remove_dir_all(&scratch);
    crate::plant(&scratch, &TREE);
    std::fs::create_dir_all(scratch.join("src/http")).expect("the scratch tree is writable");
    std::fs::write(scratch.join("src/http/nested.rs"), "fn n() { opt_c::z(); }")
        .expect("the scratch tree is writable");

    let (holes, read) = crate_holes(&scratch, &scratch);
    let holes: Vec<String> = holes.into_iter().collect();
    assert_eq!(read, 4, "lib.rs, http.rs, http/nested.rs and engine.rs");
    assert_eq!(
        holes,
        [
            "src/engine.rs :: `opt_a` is optional and no feature gating this site enables it",
            "src/engine.rs :: `opt_c` is optional and no feature gating this site enables it",
            "src/http/nested.rs :: `opt_c` is optional and no feature gating this site enables it",
        ],
    );
}
