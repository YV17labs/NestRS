//! What a port's source says an adapter over it may name: its `CrudService`, the
//! entity that service reads, and whether `crate::<port>::Entity` resolves. Read
//! with `syn` over every file the port's module tree declares, as the compiler
//! reads it, since a `use` tree names a file by more spellings than a text
//! search follows.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use syn::{Expr, ExprLit, ImplItem, Item, Lit, Meta, MetaNameValue, Type, UseTree, Visibility};

use crate::error::{CliError, CliResult, read_if_exists};
use crate::naming::Names;

/// A port's `CrudService`, whose API decides what an adapter may call.
pub(super) struct CrudPort {
    /// The service's type, which a handler injects.
    pub(super) service: String,
    /// The file implementing it.
    pub(super) file: PathBuf,
    /// The file declaring the entity it names, when its source leads to one.
    pub(super) entity: Option<PathBuf>,
}

/// What an adapter reads off a port before writing over it.
pub(super) struct Port {
    pub(super) crud: Option<CrudPort>,
    /// Whether an operation gates on the port's own `Entity`, rather than on a
    /// `#[subject]` written beside its service.
    pub(super) has_entity: bool,
}

impl Port {
    /// Read the port `names` at `root`, every module its `mod.rs` declares at any
    /// depth; of several services, the one named for it, else the first by
    /// module path.
    pub(super) fn read(root: &Path, names: &Names) -> CliResult<Self> {
        let index_file = root.join("mod.rs");
        let (flat, modules) = match parse(&index_file)? {
            Some(index) => (
                flat_entity(&index, &names.snake),
                tree(&index_file, root, index)?,
            ),
            None => (None, Vec::new()),
        };
        let mut found = Vec::new();
        for module in &modules {
            let at = At {
                module: &module.path,
                port: &names.snake,
            };
            for implemented in crud_impls(&module.items) {
                let entity = implemented
                    .entity
                    .and_then(|named| entity_file(&module.items, &named, at, flat.as_deref()))
                    .map(|relative| root.join(relative));
                found.push(CrudPort {
                    service: implemented.service,
                    file: module.file.clone(),
                    entity,
                });
            }
        }
        let named = names.service();
        let at = found.iter().position(|crud| crud.service == named);
        let crud = found.into_iter().nth(at.unwrap_or(0));
        let has_entity = crud.is_some() || flat.is_some() || root.join("entity.rs").is_file();
        Ok(Self { crud, has_entity })
    }
}

/// The source at `path`, `None` when it is absent; one that does not parse refuses
/// the run.
pub(super) fn parse(path: &Path) -> CliResult<Option<syn::File>> {
    read_if_exists(path)?
        .map(|source| syn::parse_file(&source).map_err(CliError::invalid_port_file(path)))
        .transpose()
}

/// One module of a port's tree: the file holding it, its path below the port and
/// its items, an inline module's apart from its parent's.
struct Module {
    file: PathBuf,
    path: Vec<String>,
    items: Vec<Item>,
}

/// The modules of the port whose `mod.rs` is `index_file`, parsed as `index`,
/// its children in `root`: each module it declares, inline or in a file of its
/// own at any depth, a `#[path]` followed, a file reached twice read once.
/// A file no `mod` declares is no part of the crate and is not read.
fn tree(index_file: &Path, root: &Path, index: syn::File) -> CliResult<Vec<Module>> {
    let mut modules = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(canonical(index_file));
    let mut pending = vec![(
        Module {
            file: index_file.to_path_buf(),
            path: Vec::new(),
            items: index.items,
        },
        root.to_path_buf(),
        true,
    )];
    while let Some((module, children, at_file_root)) = pending.pop() {
        for item in &module.items {
            let Item::Mod(declared) = item else {
                continue;
            };
            let name = declared.ident.to_string();
            let mut path = module.path.clone();
            path.push(name.clone());
            // At a file's top a `#[path]` is read from that file's folder, its
            // own name never added; inside an inline module, from its children's.
            let attributed = path_attribute(&declared.attrs).map(|relative| {
                let from = if at_file_root {
                    module.file.parent().unwrap_or(&children)
                } else {
                    &children
                };
                from.join(relative)
            });
            if let Some((_, items)) = &declared.content {
                // On an inline module a `#[path]` names its children's folder.
                let below = attributed.unwrap_or_else(|| children.join(&name));
                pending.push((
                    Module {
                        file: module.file.clone(),
                        path,
                        items: items.clone(),
                    },
                    below,
                    false,
                ));
                continue;
            }
            let (file, below) = match attributed {
                Some(file) => {
                    // A `#[path]` file owns its directory, as a `mod.rs` does.
                    let below = file
                        .parent()
                        .map_or_else(|| children.join(&name), Path::to_path_buf);
                    (file, below)
                }
                None => {
                    let flat = children.join(format!("{name}.rs"));
                    let file = if flat.is_file() {
                        flat
                    } else {
                        children.join(&name).join("mod.rs")
                    };
                    (file, children.join(&name))
                }
            };
            if !seen.insert(canonical(&file)) {
                continue;
            }
            let Some(source) = parse(&file)? else {
                continue;
            };
            pending.push((
                Module {
                    file,
                    path,
                    items: source.items,
                },
                below,
                true,
            ));
        }
        modules.push(module);
    }
    modules.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(modules)
}

/// `path`, resolved where it exists, so two spellings of one file are one.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The file a `#[path = "…"]` names, relative to where it is resolved.
fn path_attribute(attributes: &[syn::Attribute]) -> Option<PathBuf> {
    attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("path"))
        .find_map(|attribute| match &attribute.meta {
            Meta::NameValue(MetaNameValue {
                value:
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(path),
                        ..
                    }),
                ..
            }) => Some(PathBuf::from(path.value())),
            _ => None,
        })
}

/// One `impl CrudService for <service>`, and the path its `type Entity` spells.
struct CrudImpl {
    service: String,
    entity: Option<Vec<String>>,
}

fn crud_impls(items: &[Item]) -> Vec<CrudImpl> {
    items
        .iter()
        .filter_map(|item| {
            let Item::Impl(implemented) = item else {
                return None;
            };
            let (contract, _) = implemented.trait_.as_ref()?;
            if contract.segments.last()?.ident != "CrudService" {
                return None;
            }
            let Type::Path(service) = implemented.self_ty.as_ref() else {
                return None;
            };
            let entity = implemented.items.iter().find_map(|item| match item {
                ImplItem::Type(ty) if ty.ident == "Entity" => match &ty.ty {
                    Type::Path(entity) => Some(words(&entity.path)),
                    _ => None,
                },
                _ => None,
            });
            Some(CrudImpl {
                service: service.path.segments.last()?.ident.to_string(),
                entity,
            })
        })
        .collect()
}

fn words(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

/// A leaf of a `use` tree: the module path leading to it, and the item it names
/// and the word it binds, `None` for a glob.
struct Leaf {
    path: Vec<String>,
    item: Option<String>,
    binds: Option<String>,
    public: bool,
}

fn leaves(items: &[Item]) -> Vec<Leaf> {
    fn walk(tree: &UseTree, path: &mut Vec<String>, public: bool, out: &mut Vec<Leaf>) {
        let mut leaf = |item: Option<String>, binds: Option<String>| {
            out.push(Leaf {
                path: path.clone(),
                item,
                binds,
                public,
            });
        };
        match tree {
            UseTree::Name(name) => {
                leaf(Some(name.ident.to_string()), Some(name.ident.to_string()));
            }
            UseTree::Rename(rename) => {
                leaf(
                    Some(rename.ident.to_string()),
                    Some(rename.rename.to_string()),
                );
            }
            UseTree::Glob(_) => leaf(None, None),
            UseTree::Path(step) => {
                path.push(step.ident.to_string());
                walk(&step.tree, path, public, out);
                path.pop();
            }
            UseTree::Group(group) => {
                for tree in &group.items {
                    walk(tree, path, public, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for item in items {
        if let Item::Use(item) = item {
            let public = matches!(item.vis, Visibility::Public(_));
            walk(&item.tree, &mut Vec::new(), public, &mut out);
        }
    }
    out
}

/// Where a path is spelled: the module below the port, and the port's name.
#[derive(Clone, Copy)]
struct At<'a> {
    module: &'a [String],
    port: &'a str,
}

/// The file the entity `named` in a module's `items` lands in, relative to the
/// port: `flat` when its path reaches the port's own `Entity`, which `mod.rs`
/// re-exports, and none when its path leads out of the port, whose files an
/// adapter alone edits.
fn entity_file(
    items: &[Item],
    named: &[String],
    at: At<'_>,
    flat: Option<&Path>,
) -> Option<PathBuf> {
    let (path, item) = origin(items, named, at)?;
    let inside = inside_port(&path, at)?;
    module_file(&inside).or_else(|| {
        flat.filter(|_| item == "Entity" && inside.is_empty())
            .map(Path::to_path_buf)
    })
}

/// The module path `items` read the entity `named` from, and the item's name
/// there: `named`'s own path, or the `use` binding its one word, or else a glob,
/// which names no word.
fn origin(items: &[Item], named: &[String], at: At<'_>) -> Option<(Vec<String>, String)> {
    let (item, path) = named.split_last()?;
    if !path.is_empty() {
        return Some((path.to_vec(), item.clone()));
    }
    let leaves = leaves(items);
    if let Some(leaf) = leaves.iter().find(|leaf| leaf.binds.as_ref() == Some(item)) {
        return Some((leaf.path.clone(), leaf.item.clone()?));
    }
    let globs = || leaves.iter().filter(|leaf| leaf.binds.is_none());
    globs()
        .find(|leaf| inside_port(&leaf.path, at).is_some_and(|in_it| module_file(&in_it).is_some()))
        .or_else(|| globs().next())
        .map(|leaf| (leaf.path.clone(), item.clone()))
}

/// What `path`, spelled in the module `at`, names inside the port, from its
/// root: `crate::<port>…`, or `self` and each `super` resolved from where it is
/// spelled; `None` for a path leading out of the port.
fn inside_port(path: &[String], at: At<'_>) -> Option<Vec<String>> {
    match path {
        [root, port, inside @ ..] if root == "crate" && port == at.port => Some(inside.to_vec()),
        [first, ..] if first == "super" || first == "self" => {
            let mut resolved = at.module.to_vec();
            let mut rest = path;
            while let [word, tail @ ..] = rest {
                match word.as_str() {
                    "super" => {
                        resolved.pop()?;
                    }
                    "self" => {}
                    _ => break,
                }
                rest = tail;
            }
            resolved.extend(rest.iter().cloned());
            Some(resolved)
        }
        _ => None,
    }
}

/// The file a module path inside a port lands in: `entity.rs`, or
/// `entities/<stem>.rs`.
fn module_file(inside: &[String]) -> Option<PathBuf> {
    match inside {
        [module, ..] if module == "entity" => Some(PathBuf::from("entity.rs")),
        [module, stem, ..] if module == "entities" => {
            Some(PathBuf::from(format!("entities/{stem}.rs")))
        }
        _ => None,
    }
}

/// The file behind a port's flat `Entity`, read off its `mod.rs`, where a child is
/// named bare, behind `self` or behind `crate::<port>`: what `crate::<port>::Entity`
/// names, and `None` when it names nothing.
fn flat_entity(index: &syn::File, snake: &str) -> Option<PathBuf> {
    leaves(&index.items)
        .into_iter()
        .filter(|leaf| leaf.public && leaf.binds.as_deref().is_none_or(|word| word == "Entity"))
        .find_map(|leaf| {
            module_file(match leaf.path.as_slice() {
                [head, inside @ ..] if head == "self" => inside,
                [root, port, inside @ ..] if root == "crate" && port == snake => inside,
                path => path,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(source: &str) -> syn::File {
        syn::parse_file(source).expect("the source parses")
    }

    /// The service of `source`'s `CrudService`, and the file its entity lands in,
    /// spelled in `service.rs` of a port named `posts` whose flat `Entity` comes
    /// from `entity.rs`.
    fn crud(source: &str) -> (String, Option<PathBuf>) {
        crud_in(&["service"], source)
    }

    /// [`crud`], spelled in the module `module` below the port.
    fn crud_in(module: &[&str], source: &str) -> (String, Option<PathBuf>) {
        let source = file(source);
        let module: Vec<String> = module.iter().map(|word| (*word).to_owned()).collect();
        let at = At {
            module: &module,
            port: "posts",
        };
        let implemented = crud_impls(&source.items)
            .into_iter()
            .next()
            .expect("an impl");
        let entity = implemented
            .entity
            .and_then(|named| entity_file(&source.items, &named, at, Some(Path::new("entity.rs"))));
        (implemented.service, entity)
    }

    #[test]
    fn a_renamed_import_leads_to_the_lone_entity() {
        let (service, entity) = crud(
            "use super::entity::{CreatePost, Entity as Post};\n\
             pub struct PostsService;\n\
             impl CrudService for PostsService { type Entity = Post; }\n",
        );
        assert_eq!(service, "PostsService");
        assert_eq!(entity, Some(PathBuf::from("entity.rs")));
    }

    #[test]
    fn a_moved_entity_is_read_at_its_new_file() {
        let (_, entity) = crud(
            "use super::entities::post::{Entity as Post};\n\
             impl CrudService for PostsService { type Entity = Post; }\n",
        );
        assert_eq!(entity, Some(PathBuf::from("entities/post.rs")));
    }

    #[test]
    fn a_service_in_a_folder_reads_a_level_further_up() {
        let (_, entity) = crud_in(
            &["services", "articles"],
            "use super::super::entities::post::Entity;\n\
             impl CrudService for ArticlesService { type Entity = Entity; }\n",
        );
        assert_eq!(entity, Some(PathBuf::from("entities/post.rs")));
    }

    /// A `super` climbs from where it is spelled: from `services/articles.rs`
    /// one level up is the folder, whose `entity` is no file of the port's.
    #[test]
    fn a_super_climbs_from_the_module_it_is_spelled_in() {
        for (module, source, expected) in [
            (
                &["services", "articles"][..],
                "use super::entity::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
                None,
            ),
            (
                &["store"][..],
                "use self::super::entity::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
                Some(PathBuf::from("entity.rs")),
            ),
            (
                &[][..],
                "use super::entity::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
                None,
            ),
            (
                &[][..],
                "impl CrudService for S { type Entity = self::entity::Entity; }\n",
                Some(PathBuf::from("entity.rs")),
            ),
        ] {
            assert_eq!(crud_in(module, source).1, expected, "{module:?}: {source}");
        }
    }

    #[test]
    fn a_path_spelled_at_the_impl_is_read_there() {
        let (_, entity) = crud(
            "impl nest_rs::seaorm::CrudService for S { type Entity = super::entities::tag::Entity; }\n",
        );
        assert_eq!(entity, Some(PathBuf::from("entities/tag.rs")));
    }

    #[test]
    fn a_glob_leads_where_it_globs() {
        let (_, entity) =
            crud("use super::entity::*;\nimpl CrudService for S { type Entity = Entity; }\n");
        assert_eq!(entity, Some(PathBuf::from("entity.rs")));
    }

    #[test]
    fn the_port_s_own_entity_is_its_flat_one() {
        for source in [
            "use super::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
            "use crate::posts::Entity as Post;\nimpl CrudService for S { type Entity = Post; }\n",
            "use super::*;\nimpl CrudService for S { type Entity = Entity; }\n",
            "use crate::posts::entity::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
            "impl CrudService for S { type Entity = crate::posts::entity::Entity; }\n",
        ] {
            let (_, entity) = crud(source);
            assert_eq!(entity, Some(PathBuf::from("entity.rs")), "{source}");
        }
    }

    #[test]
    fn a_source_that_leads_elsewhere_names_no_file() {
        for source in [
            "use other::place::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
            "use crate::other::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
            "use crate::other::entity::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
            "use super::other::entity::Entity;\nimpl CrudService for S { type Entity = Entity; }\n",
            "use crate::other::entities::post::*;\nimpl CrudService for S { type Entity = Entity; }\n",
            "impl CrudService for S { type Entity = crate::other::entity::Entity; }\n",
            "impl CrudService for S { type Entity = other::entity::Entity; }\n",
            "impl CrudService for S { type Entity = Entity; }\n",
            "impl CrudService for S {}\n",
        ] {
            assert_eq!(crud(source).1, None, "{source}");
        }
    }

    /// Every module the port's tree declares, by the path the compiler gives it:
    /// a file beside `mod.rs`, a folder's `mod.rs`, a file owning a folder, an
    /// inline module, a `#[path]` on a file or on an inline module, which names
    /// its children's folder; a file no `mod` names is none of them, and each
    /// that does not parse sits where the compiler reads nothing.
    #[test]
    fn the_tree_is_every_module_the_port_declares_and_nothing_else() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        let write = |relative: &str, source: &str| {
            let file = root.join(relative);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("its folder");
            std::fs::write(file, source).expect("written");
        };
        write(
            "mod.rs",
            "mod store;\nmod services;\nmod nested;\n#[path = \"legacy/writer.rs\"]\nmod writer;\n\
             mod inline {\n    mod deeper;\n    #[path = \"moved\"]\n    mod shifted {\n        \
             mod leaf;\n    }\n}\n#[path = \"pathed\"]\nmod relocated {\n    mod item;\n}\n",
        );
        write("store.rs", "");
        write(
            "services.rs",
            "mod articles;\n#[path = \"kept\"]\nmod grouped {\n    mod one;\n}\n",
        );
        write("services/articles.rs", "");
        write("nested/mod.rs", "mod inner;\n");
        write("nested/inner.rs", "");
        write("legacy/writer.rs", "mod part;\n");
        write("legacy/part.rs", "");
        write("inline/deeper.rs", "");
        write("inline/moved/leaf.rs", "");
        write("pathed/item.rs", "");
        write("kept/one.rs", "");
        for unread in [
            "stray.rs",
            "inline/shifted/leaf.rs",
            "relocated/item.rs",
            "services/grouped/one.rs",
            "services/kept/one.rs",
        ] {
            write(unread, "fn half(\n");
        }

        let index = parse(&root.join("mod.rs"))
            .expect("parses")
            .expect("present");
        let modules = tree(&root.join("mod.rs"), root, index).expect("the tree reads");
        let read: Vec<(String, PathBuf)> = modules
            .iter()
            .map(|module| {
                (
                    module.path.join("::"),
                    module.file.strip_prefix(root).expect("below").to_path_buf(),
                )
            })
            .collect();
        let expected: Vec<(String, PathBuf)> = [
            ("", "mod.rs"),
            ("inline", "mod.rs"),
            ("inline::deeper", "inline/deeper.rs"),
            ("inline::shifted", "mod.rs"),
            ("inline::shifted::leaf", "inline/moved/leaf.rs"),
            ("nested", "nested/mod.rs"),
            ("nested::inner", "nested/inner.rs"),
            ("relocated", "mod.rs"),
            ("relocated::item", "pathed/item.rs"),
            ("services", "services.rs"),
            ("services::articles", "services/articles.rs"),
            ("services::grouped", "services.rs"),
            ("services::grouped::one", "kept/one.rs"),
            ("store", "store.rs"),
            ("writer", "legacy/writer.rs"),
            ("writer::part", "legacy/part.rs"),
        ]
        .into_iter()
        .map(|(path, file)| (path.to_owned(), PathBuf::from(file)))
        .collect();
        assert_eq!(read, expected);
    }

    #[test]
    fn a_crud_service_in_an_inline_module_is_read_where_it_is_spelled() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("mod.rs"),
            "mod entity;\npub use entity::*;\nmod write {\n    \
             impl CrudService for PostsService { type Entity = super::entity::Entity; }\n}\n",
        )
        .expect("written");
        std::fs::write(dir.path().join("entity.rs"), "").expect("written");

        let port = Port::read(dir.path(), &Names::parse("posts")).expect("the port reads");

        let crud = port.crud.expect("the inline module's service");
        assert_eq!(crud.service, "PostsService");
        assert_eq!(crud.entity, Some(dir.path().join("entity.rs")));
    }

    #[test]
    fn an_impl_of_another_trait_is_no_crud_service() {
        let source = file("impl Creatable for S { type Entity = Entity; }\n");
        assert!(crud_impls(&source.items).is_empty());
    }

    #[test]
    fn a_flat_entity_is_a_glob_or_the_name_itself_re_exported() {
        for (index, expected) in [
            ("mod entity;\npub use entity::*;\n", "entity.rs"),
            (
                "mod entities;\npub use entities::comment;\npub use entities::post::*;\n",
                "entities/post.rs",
            ),
            (
                "pub use entities::post::{Entity, Model};\n",
                "entities/post.rs",
            ),
            ("pub use self::entity::*;\n", "entity.rs"),
            (
                "pub use crate::posts::entities::post::*;\n",
                "entities/post.rs",
            ),
        ] {
            assert_eq!(
                flat_entity(&file(index), "posts"),
                Some(PathBuf::from(expected)),
                "{index}"
            );
        }
    }

    #[test]
    fn a_folder_re_exporting_modules_alone_has_no_flat_entity() {
        for index in [
            "mod entities;\npub use entities::post;\n",
            "use entities::post::*;\n",
            "pub use service::*;\n",
            "pub use other::entity::*;\n",
            "pub use super::other::entity::Entity;\n",
            "pub use crate::other::entities::post::*;\n",
        ] {
            assert_eq!(flat_entity(&file(index), "posts"), None, "{index}");
        }
    }
}
