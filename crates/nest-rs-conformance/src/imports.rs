//! A crate's `use` items, module by module: what each name a module binds
//! stands for.
//!
//! Walks a crate's module tree from its root — inline modules, and file modules
//! at the default layout — and records every `use` leaf and `extern crate` by
//! the name it binds, renamed or not, with the attributes on it. Items under
//! `#[cfg(test)]` are left out. A glob binds no name and is not recorded. A
//! `mod x;` whose file the default layout does not find, or which does not
//! parse, is in [`CrateImports::unread`] rather than skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::{Attribute, Item, UseTree, Visibility};

use crate::sources::{is_cfg_test, parsed, relative};

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

/// One module of a crate: its imports by the name they bind, and the child
/// modules it declares.
#[derive(Default)]
struct Module {
    imports: BTreeMap<String, Import>,
    children: BTreeSet<String>,
}

/// A crate's modules and their imports, read from its root.
#[derive(Default)]
pub struct CrateImports {
    modules: BTreeMap<Vec<String>, Module>,
    /// Every `mod x;` whose file the default layout does not find, or that does
    /// not parse — `"<declaring file> :: mod <x>"`.
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
            Some(file) => out.collect(Vec::new(), &file.items, src, entry, root),
            None => out
                .unread
                .push(format!("{} :: the crate root", relative(entry, root))),
        }
        out
    }

    /// Record `items` as the module at `path` — whose child files sit in `dir`,
    /// declared from `file` — and descend into its children.
    fn collect(&mut self, path: Vec<String>, items: &[Item], dir: &Path, file: &Path, root: &Path) {
        let mut module = Module::default();
        let mut children = Vec::new();
        for item in items {
            match item {
                Item::Use(item) if !is_cfg_test(&item.attrs) => {
                    let public = !matches!(item.vis, Visibility::Inherited);
                    leaves(&item.tree, &mut Vec::new(), &mut |name, target| {
                        module.imports.insert(
                            name,
                            Import {
                                path: target,
                                public,
                                attrs: item.attrs.clone(),
                            },
                        );
                    });
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
            let name = child.ident.to_string();
            let mut child_path = path.clone();
            child_path.push(name.clone());
            let child_dir = dir.join(&name);
            if let Some((_, inner)) = &child.content {
                self.collect(child_path, inner, &child_dir, file, root);
                continue;
            }
            let found: Option<PathBuf> = [dir.join(format!("{name}.rs")), child_dir.join("mod.rs")]
                .into_iter()
                .find(|candidate| candidate.is_file());
            match found
                .as_deref()
                .and_then(|found| parsed(found).map(|ast| (found, ast)))
            {
                Some((found, ast)) => self.collect(child_path, &ast.items, &child_dir, found, root),
                None => self
                    .unread
                    .push(format!("{} :: mod {name}", relative(file, root))),
            }
        }
    }

    /// The imports the module at `module` declares, by the name they bind.
    pub fn imports_of(&self, module: &[String]) -> impl Iterator<Item = (&String, &Import)> {
        self.modules
            .get(module)
            .into_iter()
            .flat_map(|module| module.imports.iter())
    }

    /// The child modules the module at `module` declares, by name.
    pub fn children_of(&self, module: &[String]) -> impl Iterator<Item = &String> {
        self.modules
            .get(module)
            .into_iter()
            .flat_map(|module| module.children.iter())
    }
}

/// Every named leaf of a `use` tree as `(name it binds, path it names)`. A glob
/// and `_` bind no name and are not reported.
fn leaves(tree: &UseTree, prefix: &mut Vec<String>, out: &mut impl FnMut(String, Vec<String>)) {
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
                    out(last.clone(), prefix.clone());
                }
            } else {
                let mut path = prefix.clone();
                path.push(ident.clone());
                out(ident, path);
            }
        }
        UseTree::Rename(rename) => {
            let alias = rename.rename.to_string();
            if alias == "_" {
                return;
            }
            let ident = rename.ident.to_string();
            let mut path = prefix.clone();
            if ident != "self" {
                path.push(ident);
            }
            out(alias, path);
        }
        UseTree::Glob(_) => {}
        UseTree::Group(group) => {
            for tree in &group.items {
                leaves(tree, prefix, out);
            }
        }
    }
}
