//! `nestrs doctor` — the toolchain report, and the variables it answers for.
//!
//! **Run with a cleared environment in an empty directory.** Doctor reads the
//! shell it runs in and the `.env` cascade of the directory it starts from, so
//! a suite inheriting the developer's own — an exported `NESTRS_ENV_PREFIX`
//! doctor calls unusable, a `.env` beside the checkout — failed or passed
//! because of that shell rather than the code. Only what finds the toolchain is
//! handed through; every other input is given by the test that needs it.

use std::path::Path;
use std::process::{Command, Output};

use crate::harness::ENV_PREFIX_VAR;

/// `nestrs doctor` in `dir`, with nothing of this process's environment but
/// what finds `rustc` and `cargo`, and `vars` on top.
fn doctor(dir: &Path, vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nestrs"));
    command.arg("doctor").current_dir(dir).env_clear();
    for toolchain in [
        "PATH",
        "HOME",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "RUSTUP_TOOLCHAIN",
    ] {
        if let Some(value) = std::env::var_os(toolchain) {
            command.env(toolchain, value);
        }
    }
    command
        .envs(vars.iter().copied())
        .output()
        .expect("doctor runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn doctor_passes_with_rust_toolchain() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let output = doctor(dir.path(), &[]);

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An unusable prefix is a blocking issue: every app started with it aborts on
/// the first name it builds.
#[test]
fn an_unusable_prefix_blocks() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let output = doctor(dir.path(), &[(ENV_PREFIX_VAR, "acme-1")]);

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(stdout(&output).contains("unusable"), "{}", stdout(&output));
}

/// A `_FILE` the loader cannot read aborts every app that reads the variable,
/// so doctor says so and blocks — it used to answer `set`. The file's path is
/// never printed, only the variable naming it.
#[test]
fn a_file_the_loader_cannot_read_blocks_naming_the_variable() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let missing = dir.path().join("secret-url");
    let file_var = "ACME_SEAORM__URL_FILE";
    let output = doctor(
        dir.path(),
        &[
            (ENV_PREFIX_VAR, "ACME"),
            (file_var, missing.to_str().expect("a UTF-8 path")),
        ],
    );

    let report = stdout(&output);
    assert!(!output.status.success(), "{report}");
    assert!(
        report.contains("ACME_SEAORM__URL: an app started here fails its boot")
            && report.contains(file_var)
            && !report.contains("secret-url"),
        "{report}"
    );
}

/// A `_FILE` naming a file holding only a line break is unset, as the loader
/// reads it — not `set`.
#[test]
fn a_file_holding_nothing_is_not_set() {
    let dir = tempfile::tempdir().expect("an empty directory");
    let empty = dir.path().join("empty");
    std::fs::write(&empty, "\n").expect("write");
    let output = doctor(
        dir.path(),
        &[
            (ENV_PREFIX_VAR, "ACME"),
            (
                "ACME_REDIS__URL_FILE",
                empty.to_str().expect("a UTF-8 path"),
            ),
        ],
    );

    let report = stdout(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("ACME_REDIS__URL: not set"), "{report}");
}
