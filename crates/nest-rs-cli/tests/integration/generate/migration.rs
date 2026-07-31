//! `nestrs g migration` — registration in both `lib.rs` and `migrator.rs`.

use crate::harness::{run_ok, write_fake_migrations_crate, write_fake_workspace};
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

    // Registered in BOTH lib.rs and migrator.rs — the whole point.
    let lib = fs::read_to_string(mig.join("lib.rs")).unwrap();
    assert!(lib.contains(&format!("mod {stem};")), "lib.rs: {lib}");

    let migrator = fs::read_to_string(mig.join("migrator.rs")).unwrap();
    // The new stem appears twice (the `use super::` import and the `Box::new`
    // vec entry) regardless of how rustfmt wrapped the file.
    assert!(
        migrator.matches(&stem).count() >= 2,
        "migrator must import and box the new migration: {migrator}"
    );
    assert!(
        migrator.contains(&format!("Box::new({stem}::Migration)")),
        "migrator vec: {migrator}"
    );
    // The pre-existing migration survives the regeneration.
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
}
