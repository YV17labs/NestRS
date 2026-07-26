//! `nestrs g migration <name>` — a SeaORM migration file, registered in **both**
//! `crates/migrations/src/lib.rs` (the `mod` line) and `migrator.rs` (the
//! `MigratorTrait` vec). `migrator.rs` is regenerated from the module list so
//! the two registrations can never drift — the one you forget by hand is the
//! one that silently never runs.

use std::path::{Path, PathBuf};

use super::cargo::{ensure_workspace_deps, migrations_deps};
use super::support::{finish, resolve_start};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_decl};
use crate::templates::migration;

pub struct MigrationOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

/// Queue the `migrations` + `seed` crates `db.just` names in every recipe, with
/// `mods` already registered in both `lib.rs` and `migrator.rs`. Called by
/// `nestrs new` with none — so `nestrs run db up` applies zero migrations on a
/// fresh tree instead of failing on a package that doesn't exist — and by
/// `g migration`'s bootstrap path with its first migration, so a workspace
/// scaffolded before these crates existed self-heals in one transaction.
pub(crate) fn queue_db_crates(s: &mut Scaffold, root: &Path, mods: &[String]) {
    s.create(
        root.join("crates/migrations/Cargo.toml"),
        migration::CRATE_CARGO.to_string(),
    );
    s.create(root.join("crates/migrations/src/lib.rs"), render_lib(mods));
    s.create(
        root.join("crates/migrations/src/migrator.rs"),
        render_migrator(mods),
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

pub fn run(opts: MigrationOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.ok_or(CliError::NotNestrsWorkspace)?;

    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);

    // A workspace predating the scaffolded crates gets them now, in the same
    // transaction as its first migration.
    let lib_path = ws.migrations_lib();
    let bootstrapping = !lib_path.is_file();

    let date = chrono::Utc::now().format("%Y%m%d").to_string();
    let existing = if bootstrapping {
        Vec::new()
    } else {
        read_migration_mods(&std::fs::read_to_string(&lib_path).map_err(CliError::Io)?)
    };
    let seq = next_seq(&existing, &date);
    let stem = format!("m{date}_{seq:06}_{}", names.snake);

    // Full, sorted module list including the new one — the single source both
    // registrations read from.
    let mut mods = existing;
    mods.push(stem.clone());
    mods.sort();
    mods.dedup();

    let r = Renderer::new(&names);
    let mut s = Scaffold::new();
    if bootstrapping {
        // `create` writes are staged in memory, so an `edit` on a file this
        // same transaction creates would read it off disk and fail — render
        // both registrations into the created files instead.
        queue_db_crates(&mut s, &ws.root, &mods);
        s.edit(
            ws.root.join("Cargo.toml"),
            ensure_workspace_deps(migrations_deps()),
        );
    }
    s.create(
        ws.migrations_root().join(format!("{stem}.rs")),
        r.render(migration::MIGRATION),
    );
    if !bootstrapping {
        // Both registrations: the `mod` line in lib.rs, and a migrator.rs
        // regenerated (overwritten) from the full module list so its vec is
        // always complete and ordered.
        s.edit(lib_path, ensure_decl(&format!("mod {stem};")));
        let migrator = render_migrator(&mods);
        s.edit(
            ws.migrations_migrator(),
            Box::new(move |_current: &str| Some(migrator.clone())),
        );
    }

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("Created migration `{stem}`"),
    )?;
    print_next_steps(&stem);
    Ok(())
}

/// Migration module names (`m<date>_<seq>_<desc>`) declared in the crate's
/// `lib.rs`, excluding the `migrator` module itself.
fn read_migration_mods(lib: &str) -> Vec<String> {
    lib.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("mod ")?.strip_suffix(';')?;
            (rest.starts_with('m') && rest != "migrator").then(|| rest.to_string())
        })
        .collect()
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
        "//! SeaORM migrations for this workspace.\n\
         //!\n\
         //! Add one with `nestrs g migration <name>`: it writes the file, the `mod` line\n\
         //! here, and regenerates `migrator.rs` from that list — the two registrations\n\
         //! cannot drift.\n\
         \n\
         {decls}mod migrator;\n\
         \n\
         pub use migrator::{{Migrator, migrate}};\n"
    )
}

/// Render `migrator.rs` from the sorted module list — the whole file, so the
/// `use super::{…}` import and the `Vec<Box<dyn MigrationTrait>>` always match
/// `lib.rs` exactly.
fn render_migrator(mods: &[String]) -> String {
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
    let boxed = mods
        .iter()
        .map(|m| format!("            Box::new({m}::Migration),\n"))
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

fn print_next_steps(stem: &str) {
    println!();
    println!("Next steps:");
    println!("  1. Fill in `crates/migrations/src/{stem}.rs` (table + columns).");
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
    fn regenerated_migrator_lists_every_module_once() {
        let out = render_migrator(&["m1_a".to_string(), "m2_b".to_string()]);
        assert!(out.contains("use super::{\n    m1_a,\n    m2_b,\n};"));
        assert!(out.contains("Box::new(m1_a::Migration),"));
        assert!(out.contains("Box::new(m2_b::Migration),"));
        assert!(out.contains("pub async fn migrate"));
    }
}
