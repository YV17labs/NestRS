//! `nestrs g migration <name>` — a SeaORM migration file, registered in **both**
//! `crates/migrations/src/lib.rs` (the `mod` line) and `migrator.rs` (the
//! `MigratorTrait` vec). `migrator.rs` is regenerated from the module list so
//! the two registrations can never drift — the one you forget by hand is the
//! one that silently never runs.

use std::path::PathBuf;

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

pub fn run(opts: MigrationOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.ok_or(CliError::NotNestrsWorkspace)?;

    let lib_path = ws.migrations_lib();
    if !lib_path.is_file() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "no `crates/migrations` crate found at {} — `g migration` writes into the SeaORM \
             migration crate; create it (a `lib.rs` + `migrator.rs`) or run from a workspace that \
             has one",
            ws.migrations_root().display()
        )));
    }

    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);

    let date = chrono::Utc::now().format("%Y%m%d").to_string();
    let existing = read_migration_mods(&std::fs::read_to_string(&lib_path).map_err(CliError::Io)?);
    let seq = next_seq(&existing, &date);
    let stem = format!("m{date}_{seq:06}_{}", names.snake);

    // Full, sorted module list including the new one — the single source the
    // regenerated `migrator.rs` reads from.
    let mut mods = existing;
    mods.push(stem.clone());
    mods.sort();
    mods.dedup();

    let r = Renderer::new(&names);
    let mut s = Scaffold::new();
    s.create(
        ws.migrations_root().join(format!("{stem}.rs")),
        r.render(migration::MIGRATION),
    );
    // Both registrations: the `mod` line in lib.rs, and a migrator.rs
    // regenerated (overwritten) from the full module list so its vec is always
    // complete and ordered.
    s.edit(lib_path, ensure_decl(&format!("mod {stem};")));
    let migrator = render_migrator(&mods);
    s.edit(
        ws.migrations_migrator(),
        Box::new(move |_current: &str| Some(migrator.clone())),
    );

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

/// Render `migrator.rs` from the sorted module list — the whole file, so the
/// `use super::{…}` import and the `Vec<Box<dyn MigrationTrait>>` always match
/// `lib.rs` exactly.
fn render_migrator(mods: &[String]) -> String {
    let imports = mods
        .iter()
        .map(|m| format!("    {m},"))
        .collect::<Vec<_>>()
        .join("\n");
    let boxed = mods
        .iter()
        .map(|m| format!("            Box::new({m}::Migration),"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "use sea_orm_migration::prelude::*;\n\
         \n\
         use super::{{\n{imports}\n}};\n\
         \n\
         pub struct Migrator;\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigratorTrait for Migrator {{\n\
         \x20   fn migrations() -> Vec<Box<dyn MigrationTrait>> {{\n\
         \x20       vec![\n{boxed}\n\
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
