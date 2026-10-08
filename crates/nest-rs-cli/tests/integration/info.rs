//! `nestrs info` — the project report, and the line it draws with `about`.

use crate::harness::{run_ok, write_fake_app, write_fake_workspace};
use std::process::Command;

#[test]
fn info_reports_the_workspace_the_cursor_stands_in() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");
    run_ok(
        dir.path(),
        &["g", "feature", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let stdout = run_ok(&app, &["info"]);

    assert!(stdout.contains("Layout:"), "{stdout}");
    assert!(stdout.contains("workspace"), "{stdout}");
    let name = dir.path().file_name().unwrap().to_string_lossy();
    assert!(stdout.contains(name.as_ref()), "{stdout}");
    assert!(
        stdout.contains("Apps:") && stdout.contains("api"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Features:") && stdout.contains("posts"),
        "{stdout}",
    );
    assert!(stdout.contains("Current app:"), "{stdout}");
    assert!(stdout.contains("Env prefix:"), "{stdout}");
    assert!(stdout.contains("Toolchain:"), "{stdout}");
}

/// The root is reported as the climb from where the reader stands, never an
/// absolute path.
#[test]
fn info_reports_the_root_relatively_and_leaks_no_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");

    let stdout = run_ok(&app, &["info"]);

    assert!(stdout.contains("Root:"), "{stdout}");
    assert!(
        stdout.contains("../.."),
        "`apps/api` is two levels down from the root: {stdout}",
    );
    assert!(
        !stdout.contains(dir.path().to_str().unwrap()),
        "the absolute root is a machine-local detail: {stdout}",
    );
}

/// Running it in the wrong directory is answered, not an error.
#[test]
fn info_says_so_plainly_outside_a_project() {
    let dir = tempfile::tempdir().unwrap();

    let stdout = run_ok(dir.path(), &["info"]);

    assert!(stdout.contains("Layout:"), "{stdout}");
    assert!(stdout.contains("not inside a nestrs"), "{stdout}");
}

/// `about` is the framework, identical on every machine; `info` is the tree in
/// front of you.
#[test]
fn info_and_about_answer_different_questions() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let about = run_ok(dir.path(), &["about"]);
    let info = run_ok(dir.path(), &["info"]);

    assert!(
        about.contains("Tagline:") && about.contains("License:"),
        "{about}"
    );
    assert!(!about.contains("Layout:"), "{about}");
    assert!(
        info.contains("Layout:") && info.contains("Features:"),
        "{info}"
    );
    assert!(!info.contains("Tagline:"), "{info}");
}

#[test]
fn info_is_listed_in_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("info"));
}
