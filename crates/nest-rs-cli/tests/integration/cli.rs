//! The bare command surface: `version`, `about`, `--help`, and `run`'s toolchain
//! probe.

use std::process::Command;

#[test]
fn version_prints_single_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("version")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();
    assert!(line.starts_with("NestRS "));
    assert!(!line.contains('\n'));
}

#[test]
fn about_prints_metadata_block() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("about")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Version:"));
    assert!(stdout.contains("Tagline:"));
    assert!(stdout.contains("Yoann Vanitou"));
}

#[test]
fn run_subcommand_is_listed_in_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("run"));
}

#[test]
fn run_without_toolchain_and_no_bootstrap_errors_clearly() {
    // Hide just/bacon/cargo from the child, so the bootstrap-disabled path must
    // report a manual-install hint.
    let empty = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["run", "--no-bootstrap", "dev"])
        .env("PATH", empty.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("just"), "stderr: {stderr}");
    assert!(stderr.contains("cargo install"), "stderr: {stderr}");
}

/// `--version` / `-V` are accepted.
#[test]
fn version_is_also_reachable_through_the_conventional_flags() {
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success(), "`nestrs {flag}` must succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.trim().starts_with("NestRS "),
            "`nestrs {flag}` must print the same line as `nestrs version`: {stdout}",
        );
    }
}
