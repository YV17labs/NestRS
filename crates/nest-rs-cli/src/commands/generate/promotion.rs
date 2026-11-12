//! A feature's lone `entity.rs` becoming the first file of `entities/`, when
//! `g entity` adds a second: the [`Promotion`] moves it, deepens its own
//! `super::` paths by the level it gains, and rewrites every path in the
//! feature naming the `entity` module — or refuses, naming the first
//! reference it cannot rewrite with certainty.
//!
//! Text, not a parse: SeaORM spells some paths inside string literals, which a
//! syntax tree leaves opaque, so every occurrence is read where it stands.

use std::path::{Path, PathBuf};

use super::support::{declares_mod, is_ident_char, words};
use crate::error::{CliError, CliResult, FileAction};
use crate::scaffold::{Scaffold, Transform, ensure_decl, ensure_lines};

/// The move of `<feature>/entity.rs` to `<feature>/entities/<stem>.rs`.
#[derive(Clone)]
pub(super) struct Promotion {
    /// The feature's directory.
    root: PathBuf,
    /// The feature's module name, as `crate::<feature>::` spells it.
    feature: String,
    /// The moved entity's file stem inside `entities/`.
    stem: String,
}

/// Why a reference cannot be rewritten with certainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Uncertain {
    /// `use super::entity;` — later paths name the module through the import.
    ModuleImported,
    /// `entity::X` in a file below the feature root: it resolves through an
    /// import the move cannot follow.
    BarePath,
    /// A `super::` path whose count does not reach the feature from its file,
    /// an inline module's, or one through the feature by another root.
    UnreachedPath,
    /// `pub mod entity;`: the module is named from outside the feature.
    VisibleModule,
    /// An inline module in the moved file, whose `super::` the move would
    /// deepen past it.
    InlineModule,
}

impl Uncertain {
    fn describe(self) -> &'static str {
        match self {
            Self::ModuleImported => "an import of the `entity` module itself",
            Self::BarePath => "a bare `entity::` path, reached through an import",
            Self::UnreachedPath => {
                "a path to an `entity` module the move cannot place from this file"
            }
            Self::VisibleModule => "a visible `entity` module, which other modules may name",
            Self::InlineModule => {
                "an inline module, whose `super::` paths would be deepened past it"
            }
        }
    }
}

impl Promotion {
    pub(super) fn new(root: &Path, feature: &str, stem: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            feature: feature.to_owned(),
            stem: stem.to_owned(),
        }
    }

    /// Where the entity lands.
    fn destination(&self) -> PathBuf {
        self.root.join("entities").join(format!("{}.rs", self.stem))
    }

    /// Queue the move, the feature's rewritten files and `entities/mod.rs`,
    /// with `mod_lines` folded into the one edit of the feature's `mod.rs`.
    /// Refuses, naming the file and line, before anything is queued when a
    /// reference cannot be rewritten with certainty.
    pub(super) fn queue(&self, s: &mut Scaffold, mod_lines: Vec<String>) -> CliResult<()> {
        let from = self.root.join("entity.rs");
        let source = read(&from)?;
        if let Some(line) = inline_module(&source) {
            return Err(self.refusal(&from, line, Uncertain::InlineModule));
        }
        self.rewrite(&source, 1)
            .map_err(|(line, why)| self.refusal(&from, line, why))?;

        let mut edits = Vec::new();
        for (path, depth) in rust_files(&self.root)? {
            if path == from {
                continue;
            }
            let source = read(&path)?;
            let rewritten = if depth == 0 {
                self.rewrite_index(&source)
            } else {
                self.rewrite(&source, depth)
            };
            rewritten.map_err(|(line, why)| self.refusal(&path, line, why))?;
            edits.push((path, depth));
        }

        let moved = self.transform(1, Some(deepen));
        s.move_to(&from, self.destination(), moved);
        for (path, depth) in edits {
            if depth == 0 {
                let promotion = self.clone();
                let lines = ensure_lines(mod_lines.clone());
                s.edit(
                    path,
                    Box::new(move |current: &str| {
                        let rewritten = promotion
                            .rewrite_index(current)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| current.to_owned());
                        let declared = declare_entities(&rewritten);
                        let out = lines(&declared).unwrap_or(declared);
                        (out != current).then_some(out)
                    }),
                );
            } else {
                s.edit(path, self.transform(depth, None));
            }
        }
        Ok(())
    }

    /// A transform rewriting a file at `depth`, then `then` over the result.
    fn transform(&self, depth: usize, then: Option<fn(&str) -> String>) -> Transform {
        let promotion = self.clone();
        Box::new(move |current: &str| {
            let rewritten = promotion
                .rewrite(current, depth)
                .ok()
                .flatten()
                .unwrap_or_else(|| current.to_owned());
            let out = match then {
                Some(f) => f(&rewritten),
                None => rewritten,
            };
            (out != current).then_some(out)
        })
    }

    fn refusal(&self, path: &Path, line: usize, why: Uncertain) -> CliError {
        CliError::EntityMoveUncertain {
            feature: self.feature.clone(),
            stem: self.stem.clone(),
            at: format!("{}:{line}", path.display()),
            what: why.describe(),
        }
    }

    /// The feature's `mod.rs`: `mod entity;` becomes `mod entities;`, and
    /// every `entity::` path it starts names `entities::<stem>::`.
    fn rewrite_index(&self, source: &str) -> Result<Option<String>, (usize, Uncertain)> {
        let mut lines = Vec::new();
        let mut changed = false;
        for (at, line) in source.split_inclusive('\n').enumerate() {
            let body = line.trim_end_matches(['\n', '\r']);
            let trimmed = body.trim();
            if trimmed == "mod entity;" {
                lines.push(line.replacen("mod entity;", "mod entities;", 1));
                changed = true;
                continue;
            }
            if trimmed.ends_with(" mod entity;") {
                return Err((at + 1, Uncertain::VisibleModule));
            }
            lines.push(line.to_owned());
        }
        let declared = lines.concat();
        match self.rewrite(&declared, 0)? {
            Some(rewritten) => Ok(Some(rewritten)),
            None => Ok(changed.then_some(declared)),
        }
    }

    /// Every path naming the feature's `entity` module from a file `depth`
    /// modules below the feature, pointed at `entities::<stem>`.
    fn rewrite(&self, source: &str, depth: usize) -> Result<Option<String>, (usize, Uncertain)> {
        let mut out = String::with_capacity(source.len());
        let mut last = 0;
        for at in words(source, "entity") {
            let segments = path_before(source, at);
            let rest = &source[at + "entity".len()..];
            let line = || source[..at].matches('\n').count() + 1;
            let through_feature =
                matches!(segments.as_slice(), [.., last] if *last == self.feature);
            let crate_path = segments == ["crate", self.feature.as_str()];
            let relative =
                !segments.is_empty() && segments.iter().all(|s| *s == "super" || *s == "self");
            if rest.starts_with("::") {
                let certain = crate_path
                    || match depth {
                        0 => segments.is_empty() || segments == ["self"],
                        n => segments.len() == n && segments.iter().all(|s| *s == "super"),
                    };
                if certain {
                    out.push_str(&source[last..at]);
                    out.push_str("entities::");
                    out.push_str(&self.stem);
                    last = at + "entity".len();
                } else if segments.is_empty() {
                    return Err((line(), Uncertain::BarePath));
                } else if relative || through_feature {
                    return Err((line(), Uncertain::UnreachedPath));
                }
            } else if relative || crate_path || through_feature || in_use_line(source, at) {
                return Err((line(), Uncertain::ModuleImported));
            }
        }
        if last == 0 {
            return Ok(None);
        }
        out.push_str(&source[last..]);
        Ok(Some(out))
    }
}

fn read(path: &Path) -> CliResult<String> {
    std::fs::read_to_string(path).map_err(CliError::file(FileAction::Read, path))
}

/// Every `.rs` file under the feature, with its module's depth below it:
/// `mod.rs` 0, `service.rs` 1, `http/mod.rs` 1, `http/controller.rs` 2.
fn rust_files(root: &Path) -> CliResult<Vec<(PathBuf, usize)>> {
    let mut found = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), 0)];
    while let Some((dir, level)) = dirs.pop() {
        let entries = std::fs::read_dir(&dir).map_err(CliError::file(FileAction::List, &dir))?;
        for entry in entries {
            let path = entry
                .map_err(CliError::file(FileAction::List, &dir))?
                .path();
            if path.is_dir() {
                dirs.push((path, level + 1));
            } else if path.extension().is_some_and(|e| e == "rs") {
                let depth = if path.file_name().is_some_and(|n| n == "mod.rs") {
                    level
                } else {
                    level + 1
                };
                found.push((path, depth));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The path segments right before `at`, read back over `segment::` pairs:
/// `super::super::` → `["super", "super"]`, nothing → `[]`.
fn path_before(source: &str, at: usize) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut end = at;
    while let Some(before) = source[..end].strip_suffix("::") {
        let start = before
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident_char(*c))
            .last()
            .map_or(before.len(), |(i, _)| i);
        if start == before.len() {
            break;
        }
        segments.push(&before[start..]);
        end = start;
    }
    segments.reverse();
    segments
}

/// Whether the line holding `at` is a `use` declaration, in any visibility.
fn in_use_line(source: &str, at: usize) -> bool {
    let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = source[start..].trim_start();
    let line = match line.strip_prefix("pub") {
        Some(rest) if rest.starts_with('(') => rest.split_once(')').map_or(rest, |(_, r)| r),
        Some(rest) => rest,
        None => line,
    };
    line.trim_start().starts_with("use ")
}

/// The first line declaring an inline module (`mod tests {`), if any.
fn inline_module(source: &str) -> Option<usize> {
    source.lines().enumerate().find_map(|(at, line)| {
        let rest = line.trim_start();
        let rest = rest.trim_prefix("pub ");
        let name = rest.strip_prefix("mod ")?;
        let name = name.trim_end();
        (!name.ends_with(';')).then_some(at + 1)
    })
}

/// Every `super::` path one level deeper, and `pub(super)` kept pointing at
/// the feature: the moved file sits one module further from it.
fn deepen(source: &str) -> String {
    let source = source.replace("pub(super)", "pub(in super::super)");
    let mut out = String::with_capacity(source.len());
    let mut last = 0;
    for at in words(&source, "super") {
        let first_of_run = !source[..at].ends_with("::");
        let in_path = source[at + "super".len()..].starts_with("::");
        let already_deepened = source[..at].ends_with("pub(in ");
        if first_of_run && in_path && !already_deepened {
            out.push_str(&source[last..at]);
            out.push_str("super::");
            last = at;
        }
    }
    out.push_str(&source[last..]);
    out
}

/// `mod entities;` declared once, for a `mod.rs` that declared no `entity`.
fn declare_entities(source: &str) -> String {
    if declares_mod(source, "entities") {
        return source.to_owned();
    }
    ensure_decl("mod entities;")(source).unwrap_or_else(|| source.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promotion() -> Promotion {
        Promotion::new(Path::new("posts"), "posts", "post")
    }

    #[test]
    fn the_moved_file_reaches_the_feature_one_level_up() {
        let source = "#[expose(name = \"Post\", service = super::service::PostsService)]\n\
                      use super::super::orgs::Entity;\n\
                      pub(super) fn helper() {}\n\
                      #[sea_orm(belongs_to = \"super::author::Entity\")]\n";
        let deepened = deepen(source);
        assert!(
            deepened.contains("service = super::super::service::PostsService"),
            "{deepened}"
        );
        assert!(
            deepened.contains("use super::super::super::orgs::Entity;"),
            "{deepened}"
        );
        assert!(
            deepened.contains("pub(in super::super) fn helper"),
            "{deepened}"
        );
        assert!(
            deepened.contains("\"super::super::author::Entity\""),
            "{deepened}"
        );
    }

    #[test]
    fn a_path_reaching_the_feature_names_the_moved_module() {
        let service = "use super::entity::{CreatePost, Entity as Posts};\n";
        assert_eq!(
            promotion().rewrite(service, 1).unwrap().as_deref(),
            Some("use super::entities::post::{CreatePost, Entity as Posts};\n")
        );
        let controller = "use super::super::entity::Model;\nuse crate::posts::entity::Column;\n";
        assert_eq!(
            promotion().rewrite(controller, 2).unwrap().as_deref(),
            Some(
                "use super::super::entities::post::Model;\nuse crate::posts::entities::post::Column;\n"
            )
        );
    }

    #[test]
    fn the_index_declares_and_re_exports_the_moved_module() {
        let index =
            "mod entity;\nmod service;\n\npub use entity::*;\npub use service::PostsService;\n";
        assert_eq!(
            promotion().rewrite_index(index).unwrap().as_deref(),
            Some(
                "mod entities;\nmod service;\n\npub use entities::post::*;\npub use service::PostsService;\n"
            )
        );
    }

    #[test]
    fn what_names_no_module_of_the_feature_is_left_alone() {
        let source = "use sea_orm::entity::prelude::*;\n\
                      let entity = load();\n\
                      // every entity is checked\n\
                      let entity_id = entity.id;\n";
        assert_eq!(promotion().rewrite(source, 1).unwrap(), None);
    }

    #[test]
    fn a_reference_it_cannot_place_is_refused_by_its_line() {
        for (source, depth, why) in [
            ("use super::entity;\n", 1, Uncertain::ModuleImported),
            (
                "\nuse super::{entity, service};\n",
                1,
                Uncertain::ModuleImported,
            ),
            ("fn f() -> entity::Model {}\n", 1, Uncertain::BarePath),
            ("use super::entity::Model;\n", 2, Uncertain::UnreachedPath),
            ("use crate::posts::entity;\n", 2, Uncertain::ModuleImported),
        ] {
            let refused = promotion().rewrite(source, depth).unwrap_err();
            let line = source.lines().position(|l| l.contains("entity")).unwrap() + 1;
            assert_eq!(refused, (line, why), "{source}");
        }
        assert_eq!(
            promotion().rewrite_index("pub mod entity;\n").unwrap_err(),
            (1, Uncertain::VisibleModule)
        );
    }

    #[test]
    fn an_inline_module_is_found_and_a_declaration_is_not() {
        assert_eq!(
            inline_module("mod entity;\n#[cfg(test)]\nmod tests {\n}\n"),
            Some(3)
        );
        assert_eq!(inline_module("pub mod http;\nmod service;\n"), None);
    }
}
