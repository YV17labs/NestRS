//! `nestrs g entity <feature>[/<name>]` — one `#[expose]`d SeaORM entity in an
//! existing feature, without the `CrudService` and guarded controller that
//! `g resource` brings with it.
//!
//! The positional is the feature; the optional `/<name>` tail names the entity
//! when the feature's singular is not it (`g entity posts/comment` ⇒ `Comment`).
//! No entity yet ⇒ the lone `entity.rs`; an `entities/` folder ⇒ one more file in
//! it. A lone `entity.rs` becoming the first of several moves into `entities/`
//! in the same transaction ([`Promotion`]), its paths rewritten — or the run is
//! refused naming the reference it cannot rewrite with certainty. A port whose
//! `#[subject]`, written by `g graphql|mcp`, holds the entity's name has it
//! retired in the same transaction, every operation gated on it re-gated on
//! the entity — or the run is refused naming the file edited beyond what the
//! generator wrote.

use std::path::{Path, PathBuf};

use super::adapter::{adapter_renderer, templates_for};
use super::cargo::{ensure_features_deps, ensure_workspace_deps, entity_deps};
use super::migration::refuse_reserved;
use super::promotion::Promotion;
use super::support::{declares_mod, finish};
use crate::column::{self, Column, table_name};
use crate::commands::resolve_start;
use crate::context::Context;
use crate::error::{CliError, CliResult, read_if_exists};
use crate::naming::{Names, Transport};
use crate::scaffold::{Renderer, Scaffold, Transform, ensure_decl, ensure_lines, rustfmt_source};
use crate::templates::{entity, feature};

pub(crate) struct EntityOptions {
    /// `<feature>` or `<feature>/<entity>`.
    pub target: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

/// Where the entity file goes — decided by the feature, never by a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Placement {
    /// `<feature>/entity.rs`: the feature owns no entity yet.
    Lone,
    /// `<feature>/entities/<stem>.rs`: the feature already owns several.
    Folder,
    /// `<feature>/entities/<stem>.rs`, the lone `entity.rs` moved beside it as
    /// `entities/<moved>.rs`: the feature owned one.
    Promote { moved: String },
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
    refuse_reserved(&target.entity.snake, &target.stem())?;
    if !ws.feature_exists(&target.feature.snake) {
        return Err(CliError::FeatureNotFound {
            name: target.feature.snake.clone(),
        });
    }

    let root = ws.feature_root(&target.feature.snake);
    let placement = placement(&root, &target.feature)?;
    if let Placement::Promote { moved } = &placement
        && *moved == target.stem()
    {
        return Err(CliError::AlreadyExists(root.join("entity.rs")));
    }
    let retirement =
        if placement == Placement::Lone && subject_declares(&root, &target.entity.entity())? {
            Some(Retirement::read(&root, &ws.root, &target.feature)?)
        } else {
            None
        };
    let stem = target.stem();

    let r = Renderer::new(&target.entity)
        .with("stem", stem.clone())
        .with("columns", column::fields(&[Column::placeholder()]));
    let mut s = Scaffold::new();

    // Read up front: `ensure_lines` compares verbatim, so an existing
    // `pub mod entities;` would gain a duplicate `mod entities;`.
    let mod_rs = root.join("mod.rs");
    let mod_src = read_if_exists(&mod_rs)?.unwrap_or_default();
    let mut port_lines = Vec::new();

    let file = match &placement {
        Placement::Lone => {
            if !declares_mod(&mod_src, "entity") {
                port_lines.push("mod entity;".to_owned());
            }
            port_lines.push("pub use entity::*;".to_owned());
            root.join("entity.rs")
        }
        Placement::Promote { moved } => {
            let index = format!("pub mod {moved};\npub mod {stem};\n");
            s.create(root.join("entities/mod.rs"), index);
            root.join(format!("entities/{stem}.rs"))
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
    match &placement {
        // The move folds the re-export into its own edit of `mod.rs`.
        Placement::Promote { moved } => Promotion::new(&root, &target.feature.snake, moved)
            .queue(&mut s, vec![format!("pub use entities::{stem};")])?,
        Placement::Lone | Placement::Folder => match &retirement {
            Some(retirement) => retirement.queue(&mut s, &root, mod_rs, port_lines),
            None => {
                s.edit(mod_rs, ensure_lines(port_lines));
            }
        },
    }

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
    print_next_steps(&target, &placement, &stem, retirement.as_ref());
    Ok(())
}

/// The port's `#[subject]`, written by `g graphql|mcp` under the name the
/// entity takes, retired as it was written: the file and its two lines in
/// `mod.rs` go, and each operation gated on it is gated on the entity.
struct Retirement {
    /// The subject's name, the port's singular.
    name: String,
    /// `mod.rs`'s two lines declaring and re-exporting it.
    lines: [String; 2],
    /// Each handler gated on it, by its path, as it reads gated on the entity.
    regated: Vec<(PathBuf, String)>,
}

impl Retirement {
    /// What retiring `feature`'s subject takes, read off the port at `root`;
    /// refused, naming the file, when the subject, its lines in `mod.rs` or a
    /// handler gated on it is no longer what the generator wrote.
    fn read(root: &Path, ws_root: &Path, feature: &Names) -> CliResult<Self> {
        let name = feature.entity();
        let edited = |file: &Path| CliError::SubjectEdited {
            feature: feature.snake.clone(),
            name: name.clone(),
            file: file
                .strip_prefix(ws_root)
                .unwrap_or(file)
                .display()
                .to_string(),
        };

        let subject = root.join("subject.rs");
        let written = Renderer::new(feature).render(feature::SUBJECT);
        if !as_generated(&subject, &written)? {
            return Err(edited(&subject));
        }
        let lines = [
            "mod subject;".to_owned(),
            format!("pub use subject::{name};"),
        ];
        let index = read_if_exists(&root.join("mod.rs"))?.unwrap_or_default();
        if !lines
            .iter()
            .all(|line| index.lines().any(|l| l.trim() == line))
        {
            return Err(edited(&root.join("mod.rs")));
        }

        let mut regated = Vec::new();
        for transport in [Transport::Graphql, Transport::Mcp] {
            let handler = root.join(transport.folder()).join(transport.handler_file());
            if !handler.is_file() {
                continue;
            }
            let (template, _) = templates_for(transport, false);
            let on_subject = adapter_renderer(feature, transport, false, false).render(template);
            if !as_generated(&handler, &on_subject)? {
                return Err(edited(&handler));
            }
            let on_entity = adapter_renderer(feature, transport, false, true).render(template);
            regated.push((handler, on_entity));
        }
        Ok(Self {
            name,
            lines,
            regated,
        })
    }

    /// Queue the retirement, `port_lines` joining `mod_rs` in its one edit.
    fn queue(&self, s: &mut Scaffold, root: &Path, mod_rs: PathBuf, port_lines: Vec<String>) {
        s.remove(root.join("subject.rs"));
        let retired = self.lines.clone();
        let declare = ensure_lines(port_lines);
        let index: Transform = Box::new(move |current: &str| {
            let kept: Vec<&str> = current
                .lines()
                .filter(|line| !retired.iter().any(|r| line.trim() == r))
                .collect();
            let mut kept = kept.join("\n");
            if current.ends_with('\n') {
                kept.push('\n');
            }
            Some(declare(&kept).unwrap_or(kept))
        });
        s.edit(mod_rs, index);
        for (handler, on_entity) in &self.regated {
            let on_entity = on_entity.clone();
            s.edit(handler, Box::new(move |_| Some(on_entity.clone())));
        }
    }
}

/// Whether `file` holds `written`, a template's rendering, as the generator
/// left it: formatted by rustfmt, or not when it could not run.
fn as_generated(file: &Path, written: &str) -> CliResult<bool> {
    let Some(on_disk) = read_if_exists(file)? else {
        return Ok(false);
    };
    Ok(on_disk == written || rustfmt_source(written).is_some_and(|formatted| on_disk == formatted))
}

/// Which of the two legal layouts this feature is in, and whether its lone
/// entity moves to make room for a second.
fn placement(root: &Path, feature: &Names) -> CliResult<Placement> {
    let lone = root.join("entity.rs");
    if lone.is_file() {
        return Ok(Placement::Promote {
            // The existing entity's new file name, read from its own `table_name`: the
            // feature's singular is wrong once someone renamed it.
            moved: existing_stem(&lone)?.unwrap_or_else(|| feature.table()),
        });
    }
    if root.join("entities").is_dir() {
        return Ok(Placement::Folder);
    }
    Ok(Placement::Lone)
}

/// The bare file name a lone `entity.rs` would take inside `entities/`, read
/// from the `#[sea_orm(table_name = "…")]` it already declares.
fn existing_stem(entity_rs: &Path) -> CliResult<Option<String>> {
    let Some(source) = read_if_exists(entity_rs)? else {
        return Ok(None);
    };
    Ok(table_name(&source))
}

/// Whether the port's `subject.rs`, written by `g graphql|mcp` over a port with
/// no entity, declares `name`: a lone entity's glob re-export of its wire type
/// would then lose to the subject's explicit one, with no error.
fn subject_declares(root: &Path, name: &str) -> CliResult<bool> {
    Ok(read_if_exists(&root.join("subject.rs"))?
        .and_then(|source| syn::parse_file(&source).ok())
        .is_some_and(|file| {
            file.items
                .iter()
                .any(|item| matches!(item, syn::Item::Struct(s) if s.ident == name))
        }))
}

fn print_next_steps(
    target: &Target,
    placement: &Placement,
    stem: &str,
    retirement: Option<&Retirement>,
) {
    let feature = &target.feature.snake;
    let (file, type_path) = match placement {
        Placement::Lone => (
            format!("crates/features/src/{feature}/entity.rs"),
            format!("crate::{feature}::Entity"),
        ),
        Placement::Folder | Placement::Promote { .. } => (
            format!("crates/features/src/{feature}/entities/{stem}.rs"),
            format!("crate::{feature}::{stem}::Entity"),
        ),
    };

    println!();
    if let Some(retirement) = retirement {
        let handlers: Vec<String> = retirement
            .regated
            .iter()
            .filter_map(|(handler, _)| {
                let edge = handler.parent()?.file_name()?.to_string_lossy();
                let file = handler.file_name()?.to_string_lossy();
                Some(format!("`{edge}/{file}`"))
            })
            .collect();
        println!(
            "`{name}` stood in for the entity in `{feature}/subject.rs`, which is retired: {}.",
            if handlers.is_empty() {
                "nothing was gated on it".to_owned()
            } else {
                format!(
                    "{} gate on `crate::{feature}::Entity` now",
                    handlers.join(" and ")
                )
            },
            name = retirement.name,
        );
        println!();
    }
    if let Placement::Promote { moved } = placement {
        println!(
            "`{feature}`'s first entity moved to `entities/{moved}.rs`, its paths rewritten: \
             `crate::{feature}::Entity` still names it."
        );
        println!();
    }
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
        Placement::Folder | Placement::Promote { .. } => {
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
    if let Some(retirement) = retirement {
        println!();
        println!(
            "     A grant of `crate::{feature}::{}` there names a subject that is gone: grant",
            retirement.name
        );
        println!("     the entity in its place, which no generator does for you:");
        println!();
        println!("       ab.can(nest_rs::authz::Action::Read, {type_path});");
    }
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
