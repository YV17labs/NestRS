//! What a port's source says an adapter over it may name: its `CrudService`, the
//! entity that service reads, and whether `crate::<port>::Entity` resolves. Read
//! with `syn`, since a `use` tree names a file by more spellings than a text
//! search follows.

use std::path::{Path, PathBuf};

use syn::{ImplItem, Item, Type, UseTree, Visibility};

use super::support::list_dir;
use crate::error::{CliResult, read_if_exists};
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
    /// Read the port `names` at `root`; of several services, the one named for
    /// it, else the first by file name.
    pub(super) fn read(root: &Path, names: &Names) -> CliResult<Self> {
        let flat = parse(&root.join("mod.rs"))?.and_then(|index| flat_entity(&index));
        let mut files = vec![root.join("service.rs")];
        files.extend(
            list_dir(&root.join("services"))?
                .into_iter()
                .filter(|file| file.extension().is_some_and(|e| e == "rs")),
        );
        let mut found = Vec::new();
        for file in files {
            let Some(source) = parse(&file)? else {
                continue;
            };
            for implemented in crud_impls(&source) {
                found.push(CrudPort {
                    service: implemented.service,
                    file: file.clone(),
                    entity: implemented
                        .entity
                        .and_then(|named| {
                            entity_file(&source, &named, &names.snake, flat.as_deref())
                        })
                        .map(|relative| root.join(relative)),
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

/// The source at `path`: `None` when it is absent or does not parse, which the
/// developer's compiler is the one to report.
fn parse(path: &Path) -> CliResult<Option<syn::File>> {
    Ok(read_if_exists(path)?.and_then(|source| syn::parse_file(&source).ok()))
}

/// One `impl CrudService for <service>`, and the path its `type Entity` spells.
struct CrudImpl {
    service: String,
    entity: Option<Vec<String>>,
}

fn crud_impls(source: &syn::File) -> Vec<CrudImpl> {
    source
        .items
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

fn leaves(source: &syn::File) -> Vec<Leaf> {
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
    for item in &source.items {
        if let Item::Use(item) = item {
            let public = matches!(item.vis, Visibility::Public(_));
            walk(&item.tree, &mut Vec::new(), public, &mut out);
        }
    }
    out
}

/// The file the entity `named` in `source` lands in, relative to the port `snake`:
/// `flat` when its path reaches the port's own `Entity`, which `mod.rs` re-exports.
fn entity_file(
    source: &syn::File,
    named: &[String],
    snake: &str,
    flat: Option<&Path>,
) -> Option<PathBuf> {
    let (path, item) = origin(source, named)?;
    module_file(&path).or_else(|| {
        let the_port = path.iter().all(|word| word == "super")
            || matches!(path.as_slice(), [root, port] if root == "crate" && port == snake);
        flat.filter(|_| item == "Entity" && the_port)
            .map(Path::to_path_buf)
    })
}

/// The module path `source` reads the entity `named` from, and the item's name
/// there: `named`'s own path, or the `use` binding its one word, or else a glob,
/// which names no word.
fn origin(source: &syn::File, named: &[String]) -> Option<(Vec<String>, String)> {
    let (item, path) = named.split_last()?;
    if !path.is_empty() {
        return Some((path.to_vec(), item.clone()));
    }
    let leaves = leaves(source);
    if let Some(leaf) = leaves.iter().find(|leaf| leaf.binds.as_ref() == Some(item)) {
        return Some((leaf.path.clone(), leaf.item.clone()?));
    }
    leaves
        .iter()
        .find(|leaf| leaf.binds.is_none() && module_file(&leaf.path).is_some())
        .or_else(|| leaves.iter().find(|leaf| leaf.binds.is_none()))
        .map(|leaf| (leaf.path.clone(), item.clone()))
}

/// The file a module path inside a port lands in: `entity.rs`, or
/// `entities/<stem>.rs`.
fn module_file(path: &[String]) -> Option<PathBuf> {
    let at = path
        .iter()
        .position(|segment| segment == "entity" || segment == "entities")?;
    if path[at] == "entity" {
        return Some(PathBuf::from("entity.rs"));
    }
    Some(PathBuf::from(format!("entities/{}.rs", path.get(at + 1)?)))
}

/// The file behind a port's flat `Entity`, read off its `mod.rs`: what
/// `crate::<port>::Entity` names, and `None` when it names nothing.
fn flat_entity(index: &syn::File) -> Option<PathBuf> {
    leaves(index)
        .into_iter()
        .filter(|leaf| leaf.public && leaf.binds.as_deref().is_none_or(|word| word == "Entity"))
        .find_map(|leaf| module_file(&leaf.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(source: &str) -> syn::File {
        syn::parse_file(source).expect("the source parses")
    }

    /// The service of `source`'s `CrudService`, and the file its entity lands in,
    /// on a port named `posts` whose flat `Entity` comes from `entity.rs`.
    fn crud(source: &str) -> (String, Option<PathBuf>) {
        let source = file(source);
        let implemented = crud_impls(&source).into_iter().next().expect("an impl");
        let entity = implemented
            .entity
            .and_then(|named| entity_file(&source, &named, "posts", Some(Path::new("entity.rs"))));
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
        let (_, entity) = crud(
            "use super::super::entities::post::Entity;\n\
             impl CrudService for ArticlesService { type Entity = Entity; }\n",
        );
        assert_eq!(entity, Some(PathBuf::from("entities/post.rs")));
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
            "impl CrudService for S { type Entity = Entity; }\n",
            "impl CrudService for S {}\n",
        ] {
            assert_eq!(crud(source).1, None, "{source}");
        }
    }

    #[test]
    fn an_impl_of_another_trait_is_no_crud_service() {
        let source = file("impl Creatable for S { type Entity = Entity; }\n");
        assert!(crud_impls(&source).is_empty());
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
        ] {
            assert_eq!(
                flat_entity(&file(index)),
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
        ] {
            assert_eq!(flat_entity(&file(index)), None, "{index}");
        }
    }
}
