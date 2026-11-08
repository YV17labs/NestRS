//! The machinery every generator shares: `--dry-run` writes nothing, and says so
//! without claiming the work happened; a file it cannot read, or a manifest it
//! cannot parse, fails it, named.

use crate::harness::{files_under, run_err, run_ok, write_fake_app, write_fake_workspace};
use std::fs;

#[test]
fn generate_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &[
            "g",
            "feature",
            "posts",
            "--dry-run",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(!dir.path().join("crates/features/src/posts").exists());
    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(!lib.contains("pub mod posts;"));
}

/// `--dry-run` never says the work happened.
#[test]
fn generate_dry_run_does_not_claim_the_work_happened() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let stdout = run_ok(
        dir.path(),
        &[
            "g",
            "feature",
            "orders",
            "--dry-run",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(stdout.contains("Dry run — no files written."), "{stdout}");
    assert!(
        !stdout.contains("Wrote feature"),
        "a dry run must not report a completed action: {stdout}",
    );
    assert!(stdout.contains("Would write feature `orders`"), "{stdout}");
}

/// An app `module.rs` the generator cannot read fails the run naming it, down
/// to the system's answer — never a run reporting the app as merely unwired.
#[test]
fn an_unreadable_app_module_fails_the_run_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "blog");
    let module_rs = app.join("src/module.rs");
    fs::remove_file(&module_rs).unwrap();
    fs::create_dir(&module_rs).unwrap();
    let before = files_under(dir.path());

    let stderr = run_err(&app, &["g", "auth"]);

    assert!(
        stderr.starts_with("error: cannot read `") && stderr.contains("apps/blog/src/module.rs`: "),
        "the refusal names the file and the system's answer: {stderr}"
    );
    assert_eq!(
        files_under(dir.path()),
        before,
        "a refused run writes nothing"
    );
}

/// A manifest a generator adds dependencies to and cannot parse is no manifest
/// it edited: the run fails naming it and where it stopped, and writes nothing,
/// where it reported success and left the dependencies out.
#[test]
fn a_manifest_that_does_not_parse_fails_the_run_naming_where() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    let manifest = dir.path().join("crates/features/Cargo.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    let line = source.lines().count() + 1;
    fs::write(&manifest, format!("{source}[half\n")).unwrap();
    let before = files_under(dir.path());

    let stderr = run_err(dir.path(), &["g", "queue", "posts", "-p", path]);

    assert!(
        stderr.contains("cannot edit the manifest `")
            && stderr.contains("crates/features/Cargo.toml`")
            && stderr.contains(&format!("line {line}, column ")),
        "the refusal names the manifest and where it stopped: {stderr}"
    );
    assert_eq!(
        files_under(dir.path()),
        before,
        "a refused run writes nothing"
    );
}
