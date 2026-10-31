//! A workspace `nestrs new` writes, the generators run over it, and the cargo
//! that builds it against this working tree — what the integration suite's
//! scaffold tests and the e2e suite's tutorial replay share.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root, from this crate's manifest directory.
pub(crate) fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repo root resolves")
}

/// Run `nestrs <args…>` with cwd at `dir` and `env` on the process, asserting
/// success, and hand back what it printed.
///
/// The environment is explicit: the CLI reads the project's env prefix from its
/// own.
pub(crate) fn nestrs(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(args)
        .current_dir(dir)
        .envs(env.iter().copied())
        .output()
        .expect("the nestrs binary runs");
    assert!(
        output.status.success(),
        "`nestrs {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Point the generated `nest-rs` requirement at this working tree.
///
/// The version under development is not on crates.io, and the registry would
/// prove yesterday's release. The umbrella alone is enough: its siblings resolve
/// through the repo's paths.
pub(crate) fn patch_to_working_tree(workspace: &Path) {
    let manifest = workspace.join("Cargo.toml");
    let mut raw = std::fs::read_to_string(&manifest).expect("the generated manifest is readable");
    raw.push_str(&format!(
        "\n[patch.crates-io]\nnest-rs = {{ path = \"{}\" }}\n",
        repo().join("crates/nest-rs").display(),
    ));
    std::fs::write(&manifest, raw).expect("the manifest is writable");
}

/// `cargo <args>` over the generated tree, with `env` on its process.
///
/// Every scaffold builds into one shared target directory; nextest runs each
/// test in its own process, so a file lock takes them one at a time.
#[expect(
    clippy::disallowed_methods,
    reason = "CARGO is the cargo that runs the suite, set by cargo itself"
)]
pub(crate) fn cargo(
    workspace: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<Output, String> {
    let target = repo().join("target/scaffold-check");
    std::fs::create_dir_all(&target).map_err(|err| format!("no target dir: {err}"))?;
    let lock = std::fs::File::create(target.join("scaffold.lock"))
        .map_err(|err| format!("no lock file: {err}"))?;
    lock.lock()
        .map_err(|err| format!("the lock is not taken: {err}"))?;
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(args)
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", &target)
        .envs(env.iter().copied())
        .output()
        .map_err(|err| format!("cargo did not run: {err}"))
}

/// `cargo clippy --workspace --all-targets --all-features -- -D warnings` over the generated
/// tree — **the gate the generated project sets for itself**.
pub(crate) fn cargo_check(workspace: &Path) -> Result<(), String> {
    let output = cargo(
        workspace,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        &[],
    )?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).into_owned())
}
