//! `nestrs g migration <name>` — a SeaORM migration file, registered in **both**
//! `crates/migrations/src/lib.rs` (the `mod` line) and `migrator.rs` (the
//! `MigratorTrait` vec). `migrator.rs` is regenerated from the module list so
//! the two registrations can never drift.
//!
//! The `nestrs_` prefix is the framework's: `g migration nestrs_outbox`
//! registers a migration `nest_rs` ships — one `migrator.rs` entry, no file —
//! and regenerating `migrator.rs` keeps every such entry it lists. Any other
//! name or table under it is refused, here and in `g resource` and `g entity`
//! ([`refuse_reserved`]).

use std::path::{Path, PathBuf};

use super::cargo::{Dep, OUTBOX, ensure_workspace_deps, migrations_deps};
use super::support::finish;
use crate::commands::resolve_start;
use crate::context::{Context, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, Transform, ensure_decl};
use crate::templates::migration;

pub(crate) struct MigrationOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

/// The prefix of the framework's tables and of the names `g migration`
/// registers them under; no migration, resource or entity of the app's takes
/// it.
const RESERVED_PREFIX: &str = "nestrs_";

/// Refuse `name` when it, or the `table` it writes, takes the
/// [`RESERVED_PREFIX`], naming the framework's migrations `g migration`
/// registers under it.
pub(crate) fn refuse_reserved(name: &str, table: &str) -> CliResult<()> {
    if !name.starts_with(RESERVED_PREFIX) && !table.starts_with(RESERVED_PREFIX) {
        return Ok(());
    }
    Err(CliError::ReservedPrefix {
        name: name.to_string(),
        accepted: FRAMEWORK
            .iter()
            .map(|known| format!("`{}`", known.name))
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// A migration `nest_rs` ships: registered in an app's migrator under its
/// reserved name, written in no file of the app's.
pub(crate) struct FrameworkMigration {
    /// The name `g migration` registers it under.
    name: &'static str,
    /// Its `MigrationName`, which places it among the app's by date.
    version: &'static str,
    /// The type the migrator boxes, by its path through the umbrella.
    path: &'static str,
    /// The umbrella features that compile [`path`](Self::path).
    dep: &'static Dep,
}

impl FrameworkMigration {
    /// The name `g migration` registers it under.
    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// Whether `migrator` lists it, by its type's own name: by path or imported.
    fn listed_in(&self, migrator: &str) -> bool {
        migrator.contains(self.path.rsplit("::").next().unwrap_or(self.path))
    }
}

/// The outbox's table, where a job pushed inside a transaction waits for its
/// commit.
pub(crate) const OUTBOX_MIGRATION: FrameworkMigration = FrameworkMigration {
    name: "nestrs_outbox",
    version: "m20261009_000001_nestrs_outbox",
    path: "nest_rs::seaorm::queue::OutboxMigration",
    dep: &OUTBOX,
};

/// Every migration the framework ships.
const FRAMEWORK: &[FrameworkMigration] = &[OUTBOX_MIGRATION];

/// What a migrator registers: the app's migration modules, and the framework's
/// migrations listed beside them.
#[derive(Default)]
pub(crate) struct Registered {
    mods: Vec<String>,
    framework: Vec<&'static FrameworkMigration>,
}

impl Registered {
    /// The app's modules `lib.rs` declares, and the framework's migrations
    /// `migrator` lists.
    fn read(lib: &str, migrator: &str) -> Self {
        Self {
            mods: read_migration_mods(lib),
            framework: FRAMEWORK
                .iter()
                .filter(|known| known.listed_in(migrator))
                .collect(),
        }
    }

    /// Every entry of `migrations()`, in the order the dates give: a module's
    /// path, or a framework migration's type.
    fn entries(&self) -> Vec<String> {
        let mut entries: Vec<(&str, String)> = self
            .mods
            .iter()
            .map(|m| (m.as_str(), format!("{m}::Migration")))
            .chain(
                self.framework
                    .iter()
                    .map(|f| (f.version, f.path.to_string())),
            )
            .collect();
        entries.sort();
        entries.into_iter().map(|(_, entry)| entry).collect()
    }
}

/// Queue the `migrations` + `seed` crates `db.just` names in every recipe,
/// registering `registered` in both `lib.rs` and `migrator.rs` — nothing from
/// `nestrs new`, the first migration from `g migration` on a workspace that
/// predates the crates.
pub(crate) fn queue_db_crates(s: &mut Scaffold, root: &Path, registered: &Registered) {
    s.create(
        root.join("crates/migrations/Cargo.toml"),
        migration::CRATE_CARGO.to_string(),
    );
    s.create(
        root.join("crates/migrations/src/lib.rs"),
        render_lib(&registered.mods),
    );
    s.create(
        root.join("crates/migrations/src/migrator.rs"),
        render_migrator(registered),
    );
    s.create(
        root.join("crates/migrations/src/bin/migrate.rs"),
        migration::CRATE_BIN.to_string(),
    );
    s.create(
        root.join("crates/seed/Cargo.toml"),
        migration::SEED_CARGO.to_string(),
    );
    s.create(
        root.join("crates/seed/src/main.rs"),
        migration::SEED_BIN.to_string(),
    );
}

/// Queue `framework`'s entry in the migrator of a workspace that keeps one,
/// and the features compiling it into `deps` — the caller's one edit of the
/// root manifest, since a file takes one edit per scaffold. Answers whether
/// the workspace keeps a migrator.
pub(crate) fn queue_framework_entry(
    s: &mut Scaffold,
    ws: &NestrsWorkspace,
    framework: &'static FrameworkMigration,
    deps: &mut Vec<&'static Dep>,
) -> CliResult<bool> {
    let lib_path = ws.migrations_lib();
    if !lib_path.is_file() {
        return Ok(false);
    }
    let lib = std::fs::read_to_string(&lib_path).map_err(CliError::Io)?;
    s.edit(ws.migrations_migrator(), regenerate(lib, Some(framework)));
    deps.push(framework.dep);
    Ok(true)
}

pub(crate) fn run(opts: MigrationOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.ok_or(CliError::NotNestrsWorkspace)?;

    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);
    let subject = crate::naming::migration_subject(&opts.name);
    match framework_named(&names.snake, &subject.snake)? {
        Some(framework) => register(&ws, framework, opts.dry_run),
        None => generate(&ws, &names, &subject, opts.dry_run),
    }
}

/// Write the app's migration `names`, about the table `subject`, and register
/// it in both files.
fn generate(ws: &NestrsWorkspace, names: &Names, subject: &Names, dry_run: bool) -> CliResult<()> {
    let lib_path = ws.migrations_lib();
    let bootstrapping = !lib_path.is_file();
    let lib = if bootstrapping {
        String::new()
    } else {
        std::fs::read_to_string(&lib_path).map_err(CliError::Io)?
    };

    let date = chrono::Utc::now().format("%Y%m%d").to_string();
    let seq = next_seq(&read_migration_mods(&lib), &date);
    let stem = format!("m{date}_{seq:06}_{}", names.snake);

    let mut s = Scaffold::new();
    s.create(
        ws.migrations_root().join(format!("{stem}.rs")),
        Renderer::new(subject).render(migration::MIGRATION),
    );
    if bootstrapping {
        // `create` writes are staged in memory, so an `edit` on a file this same
        // transaction creates would fail on disk: render into the created files.
        let registered = Registered {
            mods: vec![stem.clone()],
            ..Registered::default()
        };
        queue_db_crates(&mut s, &ws.root, &registered);
        s.edit(
            ws.root.join("Cargo.toml"),
            ensure_workspace_deps(migrations_deps()),
        );
    } else {
        s.edit(lib_path, ensure_decl(&format!("mod {stem};")));
        s.edit(
            ws.migrations_migrator(),
            regenerate(format!("{lib}\nmod {stem};\n"), None),
        );
    }

    finish(s, dry_run, &ws.root, &format!("migration `{stem}`"))?;
    print_next_steps(&stem, &subject.table());
    Ok(())
}

/// Register the framework's migration `framework`: one `migrator.rs` entry,
/// and the features compiling it.
fn register(
    ws: &NestrsWorkspace,
    framework: &'static FrameworkMigration,
    dry_run: bool,
) -> CliResult<()> {
    let migrator = ws.migrations_migrator();
    if migrator.is_file() {
        let listed = std::fs::read_to_string(&migrator).map_err(CliError::Io)?;
        if framework.listed_in(&listed) {
            println!(
                "`{}` is registered already: `migrator.rs` lists `{}`.",
                framework.name, framework.path
            );
            return Ok(());
        }
    }

    let mut s = Scaffold::new();
    let mut deps = Vec::new();
    if !queue_framework_entry(&mut s, ws, framework, &mut deps)? {
        let registered = Registered {
            framework: vec![framework],
            ..Registered::default()
        };
        queue_db_crates(&mut s, &ws.root, &registered);
        deps.extend(migrations_deps());
        deps.push(framework.dep);
    }
    s.edit(ws.root.join("Cargo.toml"), ensure_workspace_deps(deps));

    finish(
        s,
        dry_run,
        &ws.root,
        &format!("the framework's migration `{}`", framework.name),
    )?;
    println!();
    println!("Next steps:");
    println!(
        "  `{}` ships with the framework: `migrator.rs` lists it, and no file is yours.",
        framework.path
    );
    println!("  Apply it:  nestrs run db up");
    Ok(())
}

/// The framework migration a reserved `name` registers, `None` for an app's
/// own; refused when `name` or the `table` it writes takes the reserved prefix
/// for a migration the framework does not ship.
fn framework_named(name: &str, table: &str) -> CliResult<Option<&'static FrameworkMigration>> {
    if let Some(shipped) = FRAMEWORK.iter().find(|known| known.name == name) {
        return Ok(Some(shipped));
    }
    refuse_reserved(name, table)?;
    Ok(None)
}

/// Rewrite `migrator.rs` from `lib`'s module list, keeping the framework's
/// migrations it lists and adding `framework`; no change when `framework` is
/// listed already.
fn regenerate(lib: String, framework: Option<&'static FrameworkMigration>) -> Transform {
    Box::new(move |current: &str| {
        if framework.is_some_and(|f| f.listed_in(current)) {
            return None;
        }
        let mut registered = Registered::read(&lib, current);
        registered.framework.extend(framework);
        Some(render_migrator(&registered))
    })
}

/// Migration module names (`m<date>_<seq>_<desc>`) declared in the crate's
/// `lib.rs`, excluding the `migrator` module itself, sorted and once each.
fn read_migration_mods(lib: &str) -> Vec<String> {
    let mut mods: Vec<String> = lib
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("mod ")?.strip_suffix(';')?;
            (rest.starts_with('m') && rest != "migrator").then(|| rest.to_string())
        })
        .collect();
    mods.sort();
    mods.dedup();
    mods
}

/// Next 6-digit sequence for `date`, one past the highest already used that day
/// (so same-day migrations stay ordered); `000000` when it's the first.
fn next_seq(existing: &[String], date: &str) -> u32 {
    let prefix = format!("m{date}_");
    existing
        .iter()
        .filter_map(|m| m.strip_prefix(&prefix))
        .filter_map(|rest| rest.split('_').next())
        .filter_map(|seq| seq.parse::<u32>().ok())
        .max()
        .map(|max| max + 1)
        .unwrap_or(0)
}

/// Render `lib.rs` from the sorted module list — the `mod` registry `g
/// migration` appends to and [`render_migrator`] reads back.
fn render_lib(mods: &[String]) -> String {
    let decls = mods
        .iter()
        .map(|m| format!("mod {m};\n"))
        .collect::<String>();
    format!(
        "{decls}mod migrator;\n\
         \n\
         pub use migrator::{{Migrator, migrate}};\n"
    )
}

/// Render `migrator.rs` from what it registers — the whole file, so the
/// `use super::{…}` import and the vec always match `lib.rs`.
fn render_migrator(registered: &Registered) -> String {
    let mods = &registered.mods;
    // An empty crate has nothing to import; `use super::{};` would not compile.
    let imports = if mods.is_empty() {
        String::new()
    } else {
        let list = mods
            .iter()
            .map(|m| format!("    {m},"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\nuse super::{{\n{list}\n}};\n")
    };
    let boxed = registered
        .entries()
        .iter()
        .map(|entry| format!("            Box::new({entry}),\n"))
        .collect::<String>();
    format!(
        "use sea_orm_migration::prelude::*;\n\
         {imports}\
         \n\
         pub struct Migrator;\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigratorTrait for Migrator {{\n\
         \x20   fn migrations() -> Vec<Box<dyn MigrationTrait>> {{\n\
         \x20       vec![\n{boxed}\
         \x20       ]\n\
         \x20   }}\n\
         }}\n\
         \n\
         pub async fn migrate(conn: &sea_orm::DatabaseConnection) -> anyhow::Result<()> {{\n\
         \x20   Migrator::up(conn, None).await?;\n\
         \x20   Ok(())\n\
         }}\n"
    )
}

fn print_next_steps(stem: &str, table: &str) {
    println!();
    println!("Next steps:");
    println!("  1. Fill in `crates/migrations/src/{stem}.rs` (columns).");
    println!("     Add each column to the builder AND as a variant of the identifier enum;");
    println!("     it writes the `{table}` table — rename the enum if your entity disagrees,");
    println!("     or swap the create-table body for an `alter_table` on an existing one.");
    println!("  2. Apply it:  nestrs run db up   (or `db reset` to re-seed).");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_migration_mods_and_skips_migrator() {
        let lib = "mod m20260101_000000_create_a;\nmod m20260102_000000_create_b;\nmod migrator;\n\npub use migrator::Migrator;";
        assert_eq!(
            read_migration_mods(lib),
            vec!["m20260101_000000_create_a", "m20260102_000000_create_b"]
        );
    }

    #[test]
    fn next_seq_increments_within_the_day() {
        let mods = vec![
            "m20260718_000000_a".to_string(),
            "m20260718_000001_b".to_string(),
            "m20260717_000000_c".to_string(),
        ];
        assert_eq!(next_seq(&mods, "20260718"), 2);
        assert_eq!(next_seq(&mods, "20260719"), 0);
    }

    #[test]
    fn the_skeleton_names_the_table_not_the_migration() {
        let rendered = Renderer::new(&crate::naming::migration_subject("create_widgets"))
            .render(migration::MIGRATION);

        assert!(
            rendered.contains("enum Widget {") && rendered.contains(".table(Widget::Table)"),
            "the enum and the table must be the entity's, not the migration's: {rendered}"
        );
        assert!(
            !rendered.contains("CreateWidgets"),
            "a migration name must never reach the DDL as an identifier: {rendered}"
        );
        assert!(
            !rendered.contains("{{"),
            "every placeholder is substituted: {rendered}"
        );
    }

    #[test]
    fn regenerated_migrator_lists_every_module_once() {
        let out = render_migrator(&Registered {
            mods: vec!["m1_a".to_string(), "m2_b".to_string()],
            ..Registered::default()
        });
        assert!(out.contains("use super::{\n    m1_a,\n    m2_b,\n};"));
        assert!(out.contains("Box::new(m1_a::Migration),"));
        assert!(out.contains("Box::new(m2_b::Migration),"));
        assert!(out.contains("pub async fn migrate"));
    }

    /// The CLI places the framework's migration by the name the framework
    /// gives it.
    #[test]
    fn the_outbox_entry_is_the_framework_s_own_migration() {
        use sea_orm_migration::MigrationName;

        assert_eq!(
            OUTBOX_MIGRATION.version,
            nest_rs_seaorm::queue::OutboxMigration.name()
        );
    }

    #[test]
    fn a_migrator_listing_the_framework_s_migration_by_its_type_keeps_it() {
        let lib = "mod m20260101_000000_a;\nmod migrator;\n";
        let migrator = "use nest_rs::seaorm::queue::OutboxMigration;\n\
                        vec![Box::new(m20260101_000000_a::Migration), Box::new(OutboxMigration)]";
        let registered = Registered::read(lib, migrator);
        assert_eq!(
            registered.entries(),
            [
                "m20260101_000000_a::Migration",
                "nest_rs::seaorm::queue::OutboxMigration"
            ]
        );
    }

    #[test]
    fn only_the_framework_takes_the_reserved_prefix() {
        assert!(matches!(
            framework_named("nestrs_outbox", "nestrs_outbox"),
            Ok(Some(framework)) if framework.version == OUTBOX_MIGRATION.version
        ));
        assert!(matches!(
            framework_named("create_widgets", "widgets"),
            Ok(None)
        ));
        for (name, table) in [
            ("nestrs_foo", "nestrs_foo"),
            ("create_nestrs_foo", "nestrs_foo"),
        ] {
            let Err(CliError::ReservedPrefix { accepted, .. }) = framework_named(name, table)
            else {
                panic!("`{name}` takes the reserved prefix");
            };
            assert_eq!(accepted, "`nestrs_outbox`");
        }
    }

    /// A resource or an entity never writes a framework table, the outbox's
    /// included: only `g migration` registers a shipped one.
    #[test]
    fn a_table_under_the_reserved_prefix_is_refused_whoever_writes_it() {
        assert!(refuse_reserved("widget", "widget").is_ok());
        for (name, table) in [
            ("nestrs_widget", "nestrs_widget"),
            ("nestrs_outbox", "nestrs_outbox"),
        ] {
            assert!(
                matches!(
                    refuse_reserved(name, table),
                    Err(CliError::ReservedPrefix { .. })
                ),
                "`{name}` writes the table `{table}`"
            );
        }
    }
}
