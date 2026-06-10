//! First-run bootstrap of the dev toolchain.
//!
//! `just`, `bacon`, and `cargo-nextest` are tools the CLI *drives* but does not
//! bundle — Cargo deliberately has no post-install hook (the safeguard that
//! keeps Rust clear of npm-style `postinstall` supply-chain attacks), so the
//! CLI installs them on the first command that needs them instead. Detection is
//! a `--version` probe on PATH; install prefers `cargo binstall` (prebuilt,
//! fast) and falls back to `cargo install --locked`.

use std::process::{Command, Stdio};

use crate::error::{CliError, CliResult};

/// A dev tool the CLI runs but does not bundle.
struct Tool {
    /// Binary probed on PATH (e.g. `cargo-nextest`).
    bin: &'static str,
    /// Crate installed to provide it (e.g. `cargo-nextest`).
    krate: &'static str,
}

/// Installed all-at-once on first run — `just` is needed everywhere, the rest by
/// the common recipes (`bacon` for `dev`, `cargo-nextest` for `test`).
const TOOLCHAIN: &[Tool] = &[
    Tool {
        bin: "just",
        krate: "just",
    },
    Tool {
        bin: "bacon",
        krate: "bacon",
    },
    Tool {
        bin: "cargo-nextest",
        krate: "cargo-nextest",
    },
];

/// Env var that disables the first-run bootstrap (CI / offline).
const NO_BOOTSTRAP_ENV: &str = "NESTRS_NO_BOOTSTRAP";

/// Ensures every tool in [`TOOLCHAIN`] is on PATH, installing what is missing.
///
/// A no-op once everything is present. When bootstrap is disabled (the
/// `--no-bootstrap` flag or `NESTRS_NO_BOOTSTRAP`), a missing tool is a hard
/// error naming the manual install — never a silent install.
pub fn ensure_toolchain(no_bootstrap: bool) -> CliResult<()> {
    let missing: Vec<&Tool> = TOOLCHAIN
        .iter()
        .filter(|tool| !tool_available(tool.bin))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    let crates: Vec<&str> = missing.iter().map(|tool| tool.krate).collect();

    if no_bootstrap || env_disables_bootstrap() {
        // Bootstrap off (CI / offline): only `just` is mandatory — it runs the
        // recipe. `bacon`/`cargo-nextest` are recipe-specific, so let just (or
        // cargo) report them if a recipe actually invokes one. Blocking on them
        // here would refuse recipes that need neither.
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

/// Probes a binary on PATH via `--version`.
pub fn tool_available(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
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

fn binstall_available() -> bool {
    Command::new("cargo")
        .args(["binstall", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_lists_just_bacon_nextest() {
        let crates: Vec<&str> = TOOLCHAIN.iter().map(|tool| tool.krate).collect();
        assert_eq!(crates, ["just", "bacon", "cargo-nextest"]);
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
        assert!(!tool_available("nestrs-definitely-not-a-real-binary-xyz"));
    }
}
