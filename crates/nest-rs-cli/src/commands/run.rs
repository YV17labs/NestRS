//! `nestrs run <recipe> [args…]` — the single front door to project tasks.
//!
//! Forwards the recipe and its arguments verbatim to `just`, after ensuring the
//! dev toolchain is installed (see [`super::toolchain`]). The framework owns the
//! recipes shipped in the scaffolded `Justfile`; the developer edits it freely
//! and adds their own — `nestrs run <recipe>` runs both the same way.

use std::process::{Command, Stdio};

use crate::error::{CliError, CliResult};

use super::toolchain;

pub struct RunOptions {
    /// Recipe name plus trailing args forwarded to `just` (empty → list recipes).
    pub args: Vec<String>,
    /// Skip the first-run toolchain bootstrap (CI / offline).
    pub no_bootstrap: bool,
}

pub fn run(opts: RunOptions) -> CliResult<()> {
    toolchain::ensure_toolchain(opts.no_bootstrap)?;

    let mut cmd = Command::new("just");
    if opts.args.is_empty() {
        cmd.arg("--list");
    } else {
        cmd.args(&opts.args);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cmd.status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            CliError::Anyhow(anyhow::anyhow!(
                "`just` not found on PATH — run `nestrs run` again to bootstrap it, or install it with `cargo install --locked just`"
            ))
        } else {
            CliError::Io(err)
        }
    })?;

    if !status.success() {
        // Mirror just's exit code so scripts and CI see the real failure.
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
