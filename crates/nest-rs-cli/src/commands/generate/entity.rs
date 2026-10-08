//! `nestrs g entity <feature>[/<name>]` — one `#[expose]`d SeaORM entity in an
//! existing feature, without the `CrudService` and guarded controller that
//! `g resource` brings with it.
//!
//! The positional is the feature; the optional `/<name>` tail names the entity
//! when the feature's singular is not it (`g entity posts/comment` ⇒ `Comment`).
//! No entity yet ⇒ the lone `entity.rs`; an `entities/` folder ⇒ one more file in
//! it. A lone `entity.rs` becoming the first of several is **refused**
//! ([`CliError::EntitiesFolderRequired`]): moving it means rewriting `super::`
//! paths SeaORM spells inside string literals, so the refusal names the steps.

use std::path::{Path, PathBuf};

use super::cargo::{ensure_features_deps, ensure_workspace_deps, entity_deps};
use super::support::finish;
use crate::commands::resolve_start;
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_decl, ensure_lines};
use crate::templates::entity;

pub(crate) struct EntityOptions {
    /// `<feature>` or `<feature>/<entity>`.
    pub target: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

/// Where the entity file goes — decided by the feature, never by a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// `<feature>/entity.rs`: the feature owns no entity yet.
    Lone,
    /// `<feature>/entities/<stem>.rs`: the feature already owns several.
    Folder,
}

/// The two names a target carries: the feature it joins, and the entity itself.
struct Target {
    feature: Names,
    entity: Names,
}

impl Target {
    fn parse(raw: &str) -> CliResult<Self> {
        let mut segments = raw.split('/');
        let feature = segments.next().unwrap_or_default();
        let entity = segments.next().unwrap_or(feature);
        if segments.next().is_some() {
            return Err(CliError::InvalidEntityTarget(raw.to_owned()));
        }
        // The shared validator rejects a path separator, so `..` or a nested path in
        // either half never reaches the filesystem.
        for segment in [feature, entity] {
            crate::naming::validate_feature_name(segment).map_err(CliError::InvalidFeatureName)?;
        }
        Ok(Self {
            feature: Names::parse(feature),
            entity: Names::parse(entity),
        })
    }

    /// The bare file name an entity takes inside `entities/` — snake singular,
    /// so `user_identities` lands as `user_identity.rs` beside `user.rs`.
    fn stem(&self) -> String {
        self.entity.table()
    }
}

pub(crate) fn run(opts: EntityOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.ok_or(CliError::NotNestrsWorkspace)?;

    let target = Target::parse(&opts.target)?;
    if !ws.feature_exists(&target.feature.snake) {
        return Err(CliError::FeatureNotFound {
            name: target.feature.snake.clone(),
        });
    }

    let root = ws.feature_root(&target.feature.snake);
    let placement = placement(&root, &target.feature)?;
    let stem = target.stem();

    let r = Renderer::new(&target.entity).with("stem", stem.clone());
    let mut s = Scaffold::new();

    // Read up front: `ensure_lines` compares verbatim, so an existing
    // `pub mod entities;` would gain a duplicate `mod entities;`.
    let mod_rs = root.join("mod.rs");
    let mod_src = std::fs::read_to_string(&mod_rs).unwrap_or_default();
    let mut port_lines = Vec::new();

    let file = match placement {
        Placement::Lone => {
            if !declares_mod(&mod_src, "entity") {
                port_lines.push("mod entity;".to_owned());
            }
            port_lines.push("pub use entity::*;".to_owned());
            root.join("entity.rs")
        }
        Placement::Folder => {
            let index = root.join("entities/mod.rs");
            if index.is_file() {
                s.edit(index, ensure_decl(&format!("pub mod {stem};")));
            } else {
                s.create(index, r.render(entity::ENTITIES_MOD));
            }
            if !declares_mod(&mod_src, "entities") {
                port_lines.push("mod entities;".to_owned());
            }
            // The module, not a glob: two entities re-exported flat would collide on
            // `Entity`, `Model` and `Column`.
            port_lines.push(format!("pub use entities::{stem};"));
            root.join(format!("entities/{stem}.rs"))
        }
    };
    s.create(file, r.render(entity::ENTITY));
    s.edit(mod_rs, ensure_lines(port_lines));

    let deps = entity_deps();
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(deps.clone()),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(deps));

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!(
            "entity `{}` in `{}`",
            target.entity.entity(),
            target.feature.snake
        ),
    )?;
    print_next_steps(&target, placement, &stem);
    Ok(())
}

/// Which of the two legal layouts this feature is in — or the refusal, when it
/// is in the one that has no room for a second entity.
fn placement(root: &Path, feature: &Names) -> CliResult<Placement> {
    let lone = root.join("entity.rs");
    if lone.is_file() {
        return Err(CliError::EntitiesFolderRequired {
            feature: feature.snake.clone(),
            // The existing entity's new file name, read from its own `table_name`: the
            // feature's singular is wrong once someone renamed it.
            stem: existing_stem(&lone).unwrap_or_else(|| feature.table()),
        });
    }
    if root.join("entities").is_dir() {
        return Ok(Placement::Folder);
    }
    Ok(Placement::Lone)
}

/// The bare file name a lone `entity.rs` would take inside `entities/`, read
/// from the `#[sea_orm(table_name = "…")]` it already declares.
fn existing_stem(entity_rs: &Path) -> Option<String> {
    let source = std::fs::read_to_string(entity_rs).ok()?;
    let (_, rest) = source.split_once("table_name = \"")?;
    let (table, _) = rest.split_once('"')?;
    (!table.is_empty()).then(|| table.to_owned())
}

/// Whether `mod.rs` already declares `name` as a child module, in any
/// visibility.
fn declares_mod(source: &str, name: &str) -> bool {
    let decl = format!("mod {name};");
    source
        .lines()
        .map(str::trim)
        .any(|line| line == decl || line.ends_with(&format!(" {decl}")))
}

fn print_next_steps(target: &Target, placement: Placement, stem: &str) {
    let feature = &target.feature.snake;
    let (file, type_path) = match placement {
        Placement::Lone => (
            format!("crates/features/src/{feature}/entity.rs"),
            format!("crate::{feature}::Entity"),
        ),
        Placement::Folder => (
            format!("crates/features/src/{feature}/entities/{stem}.rs"),
            format!("crate::{feature}::{stem}::Entity"),
        ),
    };

    println!();
    println!("Next steps:");
    println!(
        "  1. Fill in `{file}` columns, then:  nestrs g migration create_{}",
        target.entity.snake
    );
    println!("  2. Link it to its service — `#[expose]` names none, because a `CrudService`");
    match placement {
        Placement::Lone => {
            println!("     owns exactly one entity and this feature has no such service yet. Once");
            println!(
                "     `{}` implements it, add above:",
                target.feature.service()
            );
            println!();
            println!(
                "       service = super::service::{},",
                target.feature.service()
            );
        }
        Placement::Folder => {
            println!("     owns exactly one entity and this feature's already owns another. Add");
            println!("     `service = …` above once this entity has a service of its own.");
        }
    }
    println!();
    println!("  3. Grant the ability in `define`, in `crates/features/src/authz/ability.rs`, its");
    println!("     `_ab` parameter becoming `ab`, once something reads it — `Repo` filters every");
    println!("     read by the caller's ambient `Ability`:");
    println!();
    println!("       ab.can(nest_rs::authz::Action::Manage, {type_path});");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_target_names_the_entity_after_the_feature() {
        let target = Target::parse("posts").expect("valid");
        assert_eq!(target.feature.snake, "posts");
        assert_eq!(target.entity.entity(), "Post");
        assert_eq!(target.stem(), "post");
    }

    #[test]
    fn a_slash_names_the_entity_itself() {
        let target = Target::parse("posts/user-identities").expect("valid");
        assert_eq!(target.feature.snake, "posts");
        assert_eq!(target.entity.entity(), "UserIdentity");
        assert_eq!(target.stem(), "user_identity");
    }

    #[test]
    fn a_target_is_one_slash_at_most_and_never_a_path() {
        assert!(matches!(
            Target::parse("a/b/c"),
            Err(CliError::InvalidEntityTarget(_))
        ));
        assert!(matches!(
            Target::parse("../escape"),
            Err(CliError::InvalidFeatureName(_))
        ));
        assert!(matches!(
            Target::parse("posts/.."),
            Err(CliError::InvalidFeatureName(_))
        ));
        assert!(matches!(
            Target::parse("posts/"),
            Err(CliError::InvalidFeatureName(_))
        ));
    }

    #[test]
    fn a_module_declaration_is_recognised_in_any_visibility() {
        assert!(declares_mod("mod entities;\n", "entities"));
        assert!(declares_mod("pub mod entities;\n", "entities"));
        assert!(declares_mod("pub(crate) mod entities;\n", "entities"));
        assert!(!declares_mod("mod entity;\n", "entities"));
        assert!(!declares_mod("pub use entities::post;\n", "entities"));
    }
}
