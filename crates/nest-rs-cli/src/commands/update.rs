use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};

const CRATE_NAME: &str = "nest-rs-cli";

pub struct UpdateOptions {
    /// Reinstall from `crates/nest-rs-cli` in the nestrs monorepo instead of crates.io.
    pub from_path: bool,
    /// Workspace root when using `--path` (default: auto-discover).
    pub path: Option<PathBuf>,
}

pub fn run(opts: UpdateOptions) -> CliResult<()> {
    if !cargo_available() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo is not on PATH — install Rust from https://rustup.rs"
        )));
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("install").arg("--force");

    if opts.from_path {
        let ws = match opts.path {
            Some(root) => NestrsWorkspace::require(&root)?,
            None => {
                NestrsWorkspace::discover(std::env::current_dir().map_err(CliError::Io)?.as_path())?
                    .ok_or(CliError::NotNestrsWorkspace)?
            }
        };
        let crate_path = ws.root.join("crates/nest-rs-cli");
        if !crate_path.join("Cargo.toml").is_file() {
            return Err(CliError::Anyhow(anyhow::anyhow!(
                "nest-rs-cli crate not found at {}",
                crate_path.display()
            )));
        }
        println!("Updating nestrs from {} …", crate_path.display());
        cmd.arg("--locked").arg("--path").arg(&crate_path);
    } else {
        cmd.arg(CRATE_NAME);
        println!("Updating nestrs from crates.io ({CRATE_NAME}) …");
    }

    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let status = cmd.status().map_err(CliError::Io)?;
    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo install failed — try manually: cargo install --force {CRATE_NAME}"
        )));
    }

    println!();
    println!("nestrs updated. Run `nestrs version` to confirm.");
    Ok(())
}

fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_matches_package() {
        assert_eq!(CRATE_NAME, "nest-rs-cli");
    }
}
