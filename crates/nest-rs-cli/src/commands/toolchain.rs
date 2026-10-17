//! First-run bootstrap of the dev toolchain.
//!
//! `just`, `bacon`, `cargo-nextest` and `cargo-llvm-cov` are driven but not
//! bundled — Cargo has no post-install hook — so the CLI installs them on the
//! first command that needs them: `cargo binstall`, else `cargo install --locked`.
//!
//! `llvm-tools-preview` is not here: its binaries never land on PATH, so a probe
//! would reinstall it forever. Every scaffold pins it in `rust-toolchain.toml`.

use std::process::{Command, Stdio};

use crate::error::{CliError, CliResult};

/// A dev tool the CLI runs but does not bundle.
struct Tool {
    /// Binary probed on PATH (e.g. `cargo-nextest`).
    bin: &'static str,
    /// Crate installed to provide it (e.g. `cargo-nextest`).
    krate: &'static str,
    /// Arguments that make the binary print its version and exit `0`.
    ///
    /// Carried per tool: a cargo subcommand reads its own verb back as the first
    /// argument, so `cargo-llvm-cov --version` exits `1`.
    probe: &'static [&'static str],
}

/// Installed all-at-once on first run — `just` is needed everywhere, the rest by
/// the shipped recipes (`bacon` for `dev`, `cargo-nextest` for `test`,
/// `cargo-llvm-cov` for `test cov`).
const TOOLCHAIN: &[Tool] = &[
    Tool {
        bin: "just",
        krate: "just",
        probe: &["--version"],
    },
    Tool {
        bin: "bacon",
        krate: "bacon",
        probe: &["--version"],
    },
    Tool {
        bin: "cargo-nextest",
        krate: "cargo-nextest",
        probe: &["--version"],
    },
    Tool {
        bin: "cargo-llvm-cov",
        krate: "cargo-llvm-cov",
        probe: &["llvm-cov", "--version"],
    },
];

/// Env var that disables the first-run bootstrap (CI / offline).
///
/// Not affected by a project's `--env-prefix`: it configures the `nestrs` tool.
const NO_BOOTSTRAP_ENV: &str = "NESTRS_NO_BOOTSTRAP";

/// Ensures every tool in [`TOOLCHAIN`] is on PATH, installing what is missing.
///
/// A no-op once everything is present. When bootstrap is disabled (the
/// `--no-bootstrap` flag or `NESTRS_NO_BOOTSTRAP`), a missing tool is a hard
/// error naming the manual install — never a silent install.
pub(super) fn ensure_toolchain(no_bootstrap: bool) -> CliResult<()> {
    let missing: Vec<&Tool> = TOOLCHAIN
        .iter()
        .filter(|tool| !tool_available(tool.bin, tool.probe))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    let crates: Vec<&str> = missing.iter().map(|tool| tool.krate).collect();

    if no_bootstrap || env_disables_bootstrap() {
        // Bootstrap off: only `just` is mandatory; a recipe reports the others it
        // actually invokes.
        if missing.iter().any(|tool| tool.bin == "just") {
            return Err(CliError::Anyhow(anyhow::anyhow!(
                "missing dev tools: {names}. Bootstrap is disabled — install them manually:\n  cargo install --locked {names}",
                names = crates.join(" ")
            )));
        }
        return Ok(());
    }

    install(&crates)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the CLI's own switch, read before any project exists"
)]
fn env_disables_bootstrap() -> bool {
    std::env::var(NO_BOOTSTRAP_ENV)
        .map(|value| is_truthy(&value))
        .unwrap_or(false)
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Probes a binary on PATH with the arguments that make it print its version.
pub(super) fn tool_available(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn install(crates: &[&str]) -> CliResult<()> {
    // Notice on stderr so it never pollutes a recipe's captured stdout.
    eprintln!(
        "nestrs: installing dev toolchain ({}) — first run only…",
        crates.join(", ")
    );

    let mut cmd = Command::new("cargo");
    if binstall_available() {
        cmd.args(["binstall", "--no-confirm", "--locked"]);
    } else {
        cmd.args(["install", "--locked"]);
    }
    cmd.args(crates);
    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let status = cmd.status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            CliError::Anyhow(anyhow::anyhow!(
                "cargo is not on PATH — install Rust from https://rustup.rs"
            ))
        } else {
            CliError::Io(err)
        }
    })?;

    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "toolchain install failed — install manually:\n  cargo install --locked {}",
            crates.join(" ")
        )));
    }
    Ok(())
}

/// `cargo binstall` is probed through its own verb, as every cargo subcommand is.
fn binstall_available() -> bool {
    tool_available("cargo", &["binstall", "--version"])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool a shipped recipe invokes is on the list.
    #[test]
    fn toolchain_covers_every_tool_the_shipped_recipes_invoke() {
        let crates: Vec<&str> = TOOLCHAIN.iter().map(|tool| tool.krate).collect();
        assert_eq!(crates, ["just", "bacon", "cargo-nextest", "cargo-llvm-cov"]);
    }

    /// `cargo-llvm-cov` refuses a bare `--version`, so the probe arguments stay
    /// per tool.
    #[test]
    fn a_cargo_subcommand_is_probed_through_its_verb() {
        let llvm_cov = TOOLCHAIN
            .iter()
            .find(|tool| tool.bin == "cargo-llvm-cov")
            .expect("cargo-llvm-cov is bootstrapped");
        assert_eq!(llvm_cov.probe, ["llvm-cov", "--version"]);
    }

    #[test]
    fn truthy_values_enable_opt_out() {
        for value in ["1", "true", "TRUE", "yes", "On", " true "] {
            assert!(is_truthy(value), "{value:?} should be truthy");
        }
        for value in ["", "0", "false", "no", "off"] {
            assert!(!is_truthy(value), "{value:?} should be falsy");
        }
    }

    #[test]
    fn missing_binary_probes_false() {
        assert!(!tool_available(
            "nestrs-definitely-not-a-real-binary-xyz",
            &["--version"]
        ));
    }
}
