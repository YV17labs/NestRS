//! `nestrs g migration` — registration in both `lib.rs` and `migrator.rs`.

use crate::harness::{run_err, run_ok, write_fake_migrations_crate, write_fake_workspace};
use std::fs;

#[test]
fn generate_migration_registers_in_both_lib_and_migrator() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());

    run_ok(
        dir.path(),
        &[
            "g",
            "migration",
            "create_widget",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    let mig = dir.path().join("crates/migrations/src");
    // The generated file: m<date>_<seq>_create_widget.rs (date is today).
    let generated: Vec<_> = fs::read_dir(&mig)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with("_create_widget.rs"))
        .collect();
    assert_eq!(
        generated.len(),
        1,
        "one migration file created: {generated:?}"
    );
    let stem = generated[0].trim_end_matches(".rs").to_string();

    let lib = fs::read_to_string(mig.join("lib.rs")).unwrap();
    assert!(lib.contains(&format!("mod {stem};")), "lib.rs: {lib}");

    let migrator = fs::read_to_string(mig.join("migrator.rs")).unwrap();
    // The `use super::` import and the `Box::new` entry, however rustfmt wrapped
    // the file.
    assert!(
        migrator.matches(&stem).count() >= 2,
        "migrator must import and box the new migration: {migrator}"
    );
    assert!(
        migrator.contains(&format!("Box::new({stem}::Migration)")),
        "migrator vec: {migrator}"
    );
    assert!(
        migrator.contains("Box::new(m20260101_000000_init::Migration)"),
        "migrator kept init: {migrator}"
    );
    assert!(
        migrator.contains("pub async fn migrate"),
        "migrator has migrate fn"
    );
}

#[test]
fn generate_migration_bootstraps_the_crate_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    // No `crates/migrations` — a workspace scaffolded before it shipped.
    run_ok(
        dir.path(),
        &[
            "g",
            "migration",
            "create_widget",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    let mig = dir.path().join("crates/migrations/src");
    assert!(mig.join("lib.rs").is_file());
    assert!(mig.join("bin/migrate.rs").is_file());
    assert!(dir.path().join("crates/seed/src/main.rs").is_file());

    let lib = fs::read_to_string(mig.join("lib.rs")).unwrap();
    assert!(
        lib.contains("_create_widget;"),
        "lib.rs registers it: {lib}"
    );
    let migrator = fs::read_to_string(mig.join("migrator.rs")).unwrap();
    assert!(
        migrator.contains("_create_widget::Migration)"),
        "migrator boxes it: {migrator}"
    );

    let manifest = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("\"seaorm\"") && !manifest.contains("\"http\""),
        "the migrations crate is headless, so it turns on `seaorm` alone: {manifest}"
    );
}

/// The migrations in `crates/migrations/src`, by file name.
fn migration_files(root: &std::path::Path) -> Vec<String> {
    let mut files: Vec<String> = fs::read_dir(root.join("crates/migrations/src"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('m') && n.ends_with(".rs") && n != "migrator.rs")
        .collect();
    files.sort();
    files
}

/// The workspace's `migrator.rs`.
fn read_migrator(root: &std::path::Path) -> String {
    fs::read_to_string(root.join("crates/migrations/src/migrator.rs")).unwrap()
}

const OUTBOX_ENTRY: &str = "Box::new(nest_rs::seaorm::queue::OutboxMigration)";

/// The framework's outbox migration ships in `nest_rs`: registering it is one
/// `migrator.rs` entry, no file of the app's, and the `nest-rs` features that
/// compile it.
#[test]
fn the_outbox_migration_is_registered_in_the_migrator_and_writes_no_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    let root = dir.path().to_str().unwrap();
    let lib_before = fs::read_to_string(dir.path().join("crates/migrations/src/lib.rs")).unwrap();

    run_ok(dir.path(), &["g", "migration", "nestrs_outbox", "-p", root]);

    assert_eq!(
        migration_files(dir.path()),
        ["m20260101_000000_init.rs"],
        "no migration file is written for the framework's own",
    );
    let lib = fs::read_to_string(dir.path().join("crates/migrations/src/lib.rs")).unwrap();
    assert_eq!(lib, lib_before, "lib.rs declares no module for it");
    let migrator = read_migrator(dir.path());
    assert_eq!(migrator.matches(OUTBOX_ENTRY).count(), 1, "{migrator}");
    let init = migrator
        .find("Box::new(m20260101_000000_init::Migration)")
        .unwrap_or_else(|| panic!("the app's own migration is kept: {migrator}"));
    assert!(
        init < migrator.find(OUTBOX_ENTRY).unwrap(),
        "each entry sits at its date's place: {migrator}"
    );

    let manifest = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    let nest_rs = manifest
        .lines()
        .find(|line| line.starts_with("nest-rs"))
        .unwrap_or_else(|| panic!("the workspace depends on nest-rs: {manifest}"));
    assert!(
        nest_rs.contains("\"seaorm\"") && nest_rs.contains("\"queue\""),
        "the migration compiles under `seaorm` and `queue`: {nest_rs}"
    );

    run_ok(dir.path(), &["g", "migration", "nestrs_outbox", "-p", root]);
    let again = read_migrator(dir.path());
    assert_eq!(again, migrator, "registering it twice changes nothing");
}

/// `migrator.rs` is regenerated from the `mod` list on every `g migration`;
/// the framework's entry survives it, at its date's place.
#[test]
fn a_migration_generated_later_keeps_the_framework_s_entry() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "migration", "nestrs_outbox", "-p", root]);
    run_ok(dir.path(), &["g", "migration", "create_widget", "-p", root]);

    let migrator = read_migrator(dir.path());
    assert_eq!(migrator.matches(OUTBOX_ENTRY).count(), 1, "{migrator}");
    let widget = migration_files(dir.path())
        .into_iter()
        .find(|name| name.ends_with("_create_widget.rs"))
        .expect("the app's migration is written");
    let widget_entry = format!("Box::new({}::Migration)", widget.trim_end_matches(".rs"));
    let at = |entry: &str| {
        migrator
            .find(entry)
            .unwrap_or_else(|| panic!("`{entry}` is registered: {migrator}"))
    };
    assert!(
        at("Box::new(m20260101_000000_init::Migration)") < at(OUTBOX_ENTRY)
            && at(OUTBOX_ENTRY) < at(&widget_entry),
        "the entries run in their dates' order: {migrator}"
    );
}

/// A workspace without the migrations crate gets it, listing the framework's
/// migration alone.
#[test]
fn the_outbox_migration_bootstraps_the_crate_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "migration", "nestrs_outbox", "-p", root]);

    assert!(migration_files(dir.path()).is_empty());
    let migrator = read_migrator(dir.path());
    assert!(migrator.contains(OUTBOX_ENTRY), "{migrator}");
    assert!(
        !migrator.contains("use super::"),
        "no module of the app's to import: {migrator}"
    );
}

/// The `nestrs_` prefix is the framework's: a name it does not ship, or a
/// migration whose table would take the prefix, is refused before anything is
/// written, naming the one it accepts.
#[test]
fn a_nestrs_name_the_framework_does_not_ship_is_refused() {
    for name in ["nestrs_foo", "create_nestrs_foo", "create_nestrs_outbox"] {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        write_fake_migrations_crate(dir.path());
        let root = dir.path().to_str().unwrap();
        let migrator_before = read_migrator(dir.path());

        let refused = run_err(dir.path(), &["g", "migration", name, "-p", root]);

        assert!(
            refused.contains("`nestrs_`") && refused.contains("`nestrs_outbox`"),
            "`{name}`: the refusal names the reserved prefix and the name it accepts: {refused}"
        );
        assert_eq!(migration_files(dir.path()), ["m20260101_000000_init.rs"]);
        assert_eq!(
            read_migrator(dir.path()),
            migrator_before,
            "`{name}`: nothing is written",
        );
    }
}
