//! The one reader of a crate's `use` items: what a name, written in a module,
//! stands for.
//!
//! A join reads Rust as written, and an import is the one construction that
//! makes a name mean something it does not spell: `use opt_a::Thing` puts an
//! optional dependency's item under a name no path root shows, a re-export in a
//! submodule moves it again, and `use serde_json as json` renames a crate. Each
//! join that reads a path's root had its own answer to that — or none — and
//! the joins that answered differently disagreed on the same file. This module
//! is the answer they share: it walks a crate's module tree from its root,
//! records every `use` leaf by the name it binds in its module, and resolves a
//! path through them until it roots at something no import renames — an
//! external crate, or an item of the crate itself (`crate::…`).
//!
//! **What it reads**: the crate root and every module the root's `mod` tree
//! reaches — inline or in its own file at the default layout — each `use` and
//! `extern crate` in them, renamed or not, with the attributes on it. Items
//! under `#[cfg(test)]` are left out, as every join leaves them out.
//!
//! **What it cannot read, and who refuses it**: a module at a `#[path]`, a name
//! a glob import brings in, and a name a `macro_rules!` declares. The first and
//! the third are refused in framework source by the `blinds` join; a glob is
//! answered by the join that needs its names (the `dependencies` join refuses
//! one of an optional dependency). A `mod x;` whose file the default layout
//! does not find is not skipped: it is in [`CrateImports::unread`], and a join
//! reading the tree fails on it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::{Attribute, Item, UseTree, Visibility};

use crate::sources::{is_cfg_test, parsed, relative};

/// How deep a resolution follows imports before it stops — far past any chain
/// a crate writes, and short of a cycle.
const DEPTH: usize = 16;

/// One `use` leaf (or `extern crate`): the path it names, segment by segment.
#[derive(Clone)]
pub struct Import {
    /// The path the import names, a leading `::` dropped and a `self` inside a
    /// group read as its parent.
    pub path: Vec<String>,
    /// Whether the name is re-exported outside its module (`pub` or
    /// `pub(…)`).
    pub public: bool,
    /// The attributes on the `use` item — its `#[cfg]` gates among them.
    pub attrs: Vec<Attribute>,
}

/// One module of a crate: its imports by the name they bind, the child modules
/// it declares, and the paths it glob-imports.
#[derive(Default)]
struct Module {
    imports: BTreeMap<String, Import>,
    children: BTreeSet<String>,
    globs: Vec<Vec<String>>,
}

/// A crate's modules and their imports, read from its root.
#[derive(Default)]
pub struct CrateImports {
    modules: BTreeMap<Vec<String>, Module>,
    /// Every `mod x;` whose file the default layout does not find, or that does
    /// not parse — `"<declaring file> :: mod <x>"`. Never skipped in silence: a
    /// tree read short is a tree every join reading it gets wrong.
    pub unread: Vec<String>,
}

impl CrateImports {
    /// Read the crate whose root file is `entry` — a package's `src/lib.rs`
    /// and its `src/main.rs` are two crates, with two module trees. `root` is
    /// the repository root, for the names [`CrateImports::unread`] carries.
    pub fn read(entry: &Path, root: &Path) -> Self {
        let mut out = Self::default();
        let Some(src) = entry.parent() else {
            return out;
        };
        match parsed(entry) {
            Some(file) => out.module(Vec::new(), &file.items, src, entry, root),
            None => out
                .unread
                .push(format!("{} :: the crate root", relative(entry, root))),
        }
        out
    }

    /// The imports of one item list read as a crate root — a planted file, or a
    /// join's single parsed file with no tree around it.
    pub fn of_items(items: &[Item]) -> Self {
        let mut out = Self::default();
        out.collect(Vec::new(), items, None);
        out
    }

    fn module(&mut self, path: Vec<String>, items: &[Item], dir: &Path, file: &Path, root: &Path) {
        self.collect(path, items, Some((dir, file, root)));
    }

    /// Record `items` as the module at `path`, and descend into its children —
    /// inline ones always, file ones when `tree` says where the files are.
    fn collect(&mut self, path: Vec<String>, items: &[Item], tree: Option<(&Path, &Path, &Path)>) {
        let mut module = Module::default();
        let mut children = Vec::new();
        for item in items {
            match item {
                Item::Use(item) if !is_cfg_test(&item.attrs) => {
                    let public = !matches!(item.vis, Visibility::Inherited);
                    leaves(
                        &item.tree,
                        &mut Vec::new(),
                        &mut |name, target| match name {
                            Some(name) => {
                                module.imports.insert(
                                    name,
                                    Import {
                                        path: target,
                                        public,
                                        attrs: item.attrs.clone(),
                                    },
                                );
                            }
                            None => module.globs.push(target),
                        },
                    );
                }
                Item::ExternCrate(item) if !is_cfg_test(&item.attrs) => {
                    let name = item
                        .rename
                        .as_ref()
                        .map_or_else(|| item.ident.to_string(), |(_, rename)| rename.to_string());
                    module.imports.insert(
                        name,
                        Import {
                            path: vec![item.ident.to_string()],
                            public: !matches!(item.vis, Visibility::Inherited),
                            attrs: item.attrs.clone(),
                        },
                    );
                }
                Item::Mod(child) if !is_cfg_test(&child.attrs) => {
                    module.children.insert(child.ident.to_string());
                    children.push(child);
                }
                _ => {}
            }
        }
        self.modules.insert(path.clone(), module);
        for child in children {
            let mut child_path = path.clone();
            child_path.push(child.ident.to_string());
            match (&child.content, tree) {
                (Some((_, inner)), Some((dir, file, root))) => {
                    let child_dir = dir.join(child.ident.to_string());
                    self.collect(child_path, inner, Some((&child_dir, file, root)));
                }
                (Some((_, inner)), None) => self.collect(child_path, inner, None),
                (None, Some((dir, file, root))) => {
                    let name = child.ident.to_string();
                    let flat = dir.join(format!("{name}.rs"));
                    let nested = dir.join(&name).join("mod.rs");
                    let found: Option<PathBuf> = [flat, nested]
                        .into_iter()
                        .find(|candidate| candidate.is_file());
                    match found
                        .as_deref()
                        .and_then(|found| parsed(found).map(|ast| (found, ast)))
                    {
                        Some((found, ast)) => {
                            let child_dir = dir.join(&name);
                            self.collect(child_path, &ast.items, Some((&child_dir, found, root)));
                        }
                        None => self
                            .unread
                            .push(format!("{} :: mod {name}", relative(file, root))),
                    }
                }
                (None, None) => {}
            }
        }
    }

    /// What `path`, written in the module at `module` (`[]` the crate root),
    /// names once every import on the way is read through.
    ///
    /// The answer roots at an external crate (`["opt_a", "Thing"]`), at an item
    /// of this crate (`["crate", "a", "Thing"]`), or — for a name no import, no
    /// child module and no `crate`/`self`/`super` explains — at the name as
    /// written: a local binding, a prelude name, or a crate in the extern
    /// prelude, which is the same answer read off the spelling.
    pub fn resolve(&self, module: &[String], path: &[String]) -> Vec<String> {
        self.resolve_at(module, path, 0)
    }

    fn resolve_at(&self, module: &[String], path: &[String], depth: usize) -> Vec<String> {
        let Some(first) = path.first() else {
            return Vec::new();
        };
        if depth > DEPTH {
            return path.to_vec();
        }
        match first.as_str() {
            "crate" => self.walk(&[], &path[1..], depth),
            "self" => self.walk(module, &path[1..], depth),
            "super" => {
                let parent = &module[..module.len().saturating_sub(1)];
                self.resolve_at(parent, &relabel(&path[1..]), depth + 1)
            }
            _ => {
                let Some(here) = self.modules.get(module) else {
                    return path.to_vec();
                };
                if let Some(import) = here.imports.get(first) {
                    // `use serde_json;` binds the crate to its own name: read
                    // through, it is the crate.
                    if import.path.len() == 1 && import.path[0] == *first {
                        return path.to_vec();
                    }
                    let mut through = import.path.clone();
                    through.extend_from_slice(&path[1..]);
                    return self.resolve_at(module, &through, depth + 1);
                }
                if here.children.contains(first) {
                    let mut child = module.to_vec();
                    child.push(first.clone());
                    return self.walk(&child, &path[1..], depth);
                }
                path.to_vec()
            }
        }
    }

    /// `rest`, read inside the module at `module`.
    fn walk(&self, module: &[String], rest: &[String], depth: usize) -> Vec<String> {
        let canonical = || {
            let mut out = vec!["crate".to_owned()];
            out.extend(module.iter().cloned());
            out.extend(rest.iter().cloned());
            out
        };
        let Some(name) = rest.first() else {
            return canonical();
        };
        let Some(here) = self.modules.get(module) else {
            return canonical();
        };
        if let Some(import) = here.imports.get(name) {
            let mut through = import.path.clone();
            through.extend_from_slice(&rest[1..]);
            return self.resolve_at(module, &through, depth + 1);
        }
        if here.children.contains(name) {
            let mut child = module.to_vec();
            child.push(name.clone());
            return self.walk(&child, &rest[1..], depth);
        }
        canonical()
    }

    /// The imports the module at `module` declares, by the name they bind.
    pub fn imports_of(&self, module: &[String]) -> impl Iterator<Item = (&String, &Import)> {
        self.modules
            .get(module)
            .into_iter()
            .flat_map(|module| module.imports.iter())
    }

    /// Whether the crate's module tree holds a module at `module` — a file at
    /// the default layout that no `mod` declares (a CLI template, a fixture
    /// compiled by `include!`) is not part of the crate.
    pub fn has_module(&self, module: &[String]) -> bool {
        self.modules.contains_key(module)
    }

    /// The child modules the module at `module` declares, by name.
    pub fn children_of(&self, module: &[String]) -> impl Iterator<Item = &String> {
        self.modules
            .get(module)
            .into_iter()
            .flat_map(|module| module.children.iter())
    }

    /// The paths the module at `module` glob-imports (`use x::*` is `["x"]`).
    pub fn globs_of(&self, module: &[String]) -> &[Vec<String>] {
        self.modules
            .get(module)
            .map_or(&[], |module| module.globs.as_slice())
    }
}

/// The module a file at the default layout is, below its crate's `src/`:
/// `src/a/b.rs` and `src/a/b/mod.rs` are `["a", "b"]`, `src/lib.rs` and
/// `src/main.rs` are the root. `None` for a file outside `src/` or under
/// `src/bin/`, which is the root of a crate of its own.
pub fn module_of(file: &Path, src: &Path) -> Option<Vec<String>> {
    let below = file.strip_prefix(src).ok()?;
    let mut parts: Vec<String> = below
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.first().is_some_and(|first| first == "bin") {
        return None;
    }
    let last = parts.pop()?;
    match last.as_str() {
        "lib.rs" | "main.rs" if parts.is_empty() => Some(Vec::new()),
        "mod.rs" => Some(parts),
        _ => {
            parts.push(last.strip_suffix(".rs")?.to_owned());
            Some(parts)
        }
    }
}

/// Every path a token stream spells — each maximal run `a::b::c`, a single
/// identifier included — that is not a method or a field reached through `.`,
/// at any depth. A leading `::` is kept as an empty first segment, which no
/// import binds.
pub fn spelled_paths(tokens: proc_macro2::TokenStream) -> Vec<Vec<String>> {
    use proc_macro2::{Spacing, TokenTree};
    let mut flat = Vec::new();
    crate::sources::flatten(tokens, &mut flat);
    let mut out = Vec::new();
    let mut at = 0;
    while at < flat.len() {
        let TokenTree::Ident(first) = &flat[at] else {
            at += 1;
            continue;
        };
        let after_dot =
            at > 0 && matches!(&flat[at - 1], TokenTree::Punct(p) if p.as_char() == '.');
        let after_colons = at > 1
            && matches!(&flat[at - 1], TokenTree::Punct(p) if p.as_char() == ':')
            && matches!(&flat[at - 2], TokenTree::Punct(p) if p.as_char() == ':' && p.spacing() == Spacing::Joint);
        let mut path = Vec::new();
        if after_colons {
            path.push(String::new());
        }
        path.push(first.to_string());
        let mut next = at + 1;
        while let (
            Some(TokenTree::Punct(a)),
            Some(TokenTree::Punct(b)),
            Some(TokenTree::Ident(segment)),
        ) = (flat.get(next), flat.get(next + 1), flat.get(next + 2))
        {
            if a.as_char() != ':' || a.spacing() != Spacing::Joint || b.as_char() != ':' {
                break;
            }
            path.push(segment.to_string());
            next += 3;
        }
        if !after_dot {
            out.push(path);
        }
        at = next;
    }
    out
}

/// `super::super::x` is `super` read twice: the path after the first `super`,
/// unchanged — the caller already stepped to the parent.
fn relabel(rest: &[String]) -> Vec<String> {
    match rest.first().map(String::as_str) {
        Some("super") => {
            let mut out = vec!["super".to_owned()];
            out.extend_from_slice(&rest[1..]);
            out
        }
        _ => {
            let mut out = vec!["self".to_owned()];
            out.extend_from_slice(rest);
            out
        }
    }
}

/// Every leaf of a `use` tree as `(name it binds, path it names)` — `None` for
/// a glob, whose path is the prefix the `*` sits under. `_` binds nothing and
/// is not reported.
fn leaves(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    out: &mut impl FnMut(Option<String>, Vec<String>),
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            leaves(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let ident = name.ident.to_string();
            if ident == "self" {
                if let Some(last) = prefix.last() {
                    out(Some(last.clone()), prefix.clone());
                }
            } else {
                let mut path = prefix.clone();
                path.push(ident.clone());
                out(Some(ident), path);
            }
        }
        UseTree::Rename(rename) => {
            let alias = rename.rename.to_string();
            if alias == "_" {
                return;
            }
            let ident = rename.ident.to_string();
            let path = if ident == "self" {
                prefix.clone()
            } else {
                let mut path = prefix.clone();
                path.push(ident);
                path
            };
            out(Some(alias), path);
        }
        UseTree::Glob(_) => out(None, prefix.clone()),
        UseTree::Group(group) => {
            for tree in &group.items {
                leaves(tree, prefix, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> Vec<String> {
        text.split("::").map(str::to_owned).collect()
    }

    /// Every shape an import takes is read through to where it roots.
    #[test]
    fn a_name_is_read_through_every_import_on_the_way() {
        let file: syn::File = syn::parse_quote! {
            use opt_a::Thing;
            use serde_json as json;
            use opt_b::{self as b, nested::{Deep as Shallow}};
            extern crate opt_c as c;
            pub(crate) use opt_d::Rooted;
            mod a {
                pub use opt_e::Moved;
                pub use super::Thing as Again;
                pub mod b { pub use crate::a::Moved as Twice; }
            }
            use a::b::Twice;
            #[cfg(test)]
            use opt_f::Test;
        };
        let imports = CrateImports::of_items(&file.items);
        let root: &[String] = &[];
        let resolve = |module: &[String], written: &str| imports.resolve(module, &path(written));
        assert_eq!(resolve(root, "Thing::new"), path("opt_a::Thing::new"));
        assert_eq!(
            resolve(root, "json::from_str"),
            path("serde_json::from_str")
        );
        assert_eq!(resolve(root, "b::x"), path("opt_b::x"));
        assert_eq!(resolve(root, "Shallow"), path("opt_b::nested::Deep"));
        assert_eq!(resolve(root, "c::y"), path("opt_c::y"));
        assert_eq!(resolve(root, "crate::Rooted"), path("opt_d::Rooted"));
        assert_eq!(resolve(root, "a::Moved"), path("opt_e::Moved"));
        assert_eq!(resolve(root, "crate::a::Again"), path("opt_a::Thing"));
        assert_eq!(resolve(root, "Twice"), path("opt_e::Moved"));
        assert_eq!(
            resolve(&["a".to_owned()], "super::Rooted"),
            path("opt_d::Rooted")
        );
        assert_eq!(
            resolve(&["a".to_owned(), "b".to_owned()], "super::super::json"),
            path("serde_json")
        );
        // An item of the crate roots at `crate`; an unknown name stays as
        // written; a `#[cfg(test)]` import is not read.
        assert_eq!(resolve(root, "a::Local"), path("crate::a::Local"));
        assert_eq!(resolve(root, "tokio::spawn"), path("tokio::spawn"));
        assert_eq!(resolve(root, "Test"), path("Test"));
    }

    /// A glob is recorded by its prefix, never read as a name.
    #[test]
    fn a_glob_is_recorded_by_the_prefix_it_opens() {
        let file: syn::File = syn::parse_quote! {
            use opt_a::prelude::*;
            use opt_b::{Capability::*, Other};
        };
        let imports = CrateImports::of_items(&file.items);
        assert_eq!(
            imports.globs_of(&[]),
            [path("opt_a::prelude"), path("opt_b::Capability")]
        );
    }
}
