//! `nestrs lint` — the naming rule, as the command a project's CI runs.
//!
//! The rule is unit-tested beside its code; these prove the exit codes a CI
//! depends on.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn project(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("crates/features/src");
    fs::create_dir_all(&src).unwrap();
    for (name, body) in files {
        fs::write(src.join(name), body).unwrap();
    }
    dir
}

fn lint(dir: &TempDir) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["lint", "--path"])
        .arg(dir.path())
        .output()
        .unwrap()
}

#[test]
fn a_tree_named_for_what_it_declares_exits_zero() {
    let dir = project(&[("connection.rs", "pub struct RedisConnection;")]);
    let out = lint(&dir);

    assert!(
        out.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 files checked"));
}

#[test]
fn a_file_named_for_a_slot_fails_the_command() {
    let dir = project(&[("principal.rs", "pub struct DeskOperator;")]);
    let out = lint(&dir);

    assert!(!out.status.success(), "a printed lint nobody's CI can fail");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("principal"), "{said}");
    assert!(said.contains("DeskOperator"), "{said}");
}
