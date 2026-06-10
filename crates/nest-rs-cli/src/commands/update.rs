use std::cmp::Ordering;
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
    /// Reinstall even when already on the latest version (passes `--force` to cargo).
    pub force: bool,
}

pub fn run(opts: UpdateOptions) -> CliResult<()> {
    if !cargo_available() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo is not on PATH — install Rust from https://rustup.rs"
        )));
    }

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
        return run_path_install(&crate_path);
    }

    let current = env!("CARGO_PKG_VERSION");

    if !opts.force {
        let latest = latest_crates_io_version()?;
        match version_cmp(current, &latest) {
            Ordering::Less => {
                println!("Updating nestrs {current} → {latest} from crates.io …");
            }
            Ordering::Equal => {
                println!("nestrs {current} is already the latest version.");
                println!("Use `nestrs update --force` to reinstall anyway.");
                return Ok(());
            }
            Ordering::Greater => {
                println!(
                    "nestrs {current} is newer than {latest} on crates.io — no update available."
                );
                println!("Use `nestrs update --force` to reinstall anyway.");
                return Ok(());
            }
        }
    } else {
        println!("Reinstalling nestrs {current} from crates.io …");
    }

    run_crates_io_install(opts.force)
}

fn run_crates_io_install(force: bool) -> CliResult<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("install");
    if force {
        cmd.arg("--force");
    }
    cmd.arg(CRATE_NAME);

    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let status = cmd.status().map_err(CliError::Io)?;
    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo install failed — try manually: cargo install {CRATE_NAME}"
        )));
    }

    println!();
    println!("nestrs updated. Run `nestrs version` to confirm.");
    Ok(())
}

fn run_path_install(crate_path: &std::path::Path) -> CliResult<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("install")
        .arg("--force")
        .arg("--locked")
        .arg("--path")
        .arg(crate_path);

    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let status = cmd.status().map_err(CliError::Io)?;
    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo install failed — try manually: cargo install --locked --path {}",
            crate_path.display()
        )));
    }

    println!();
    println!("nestrs updated. Run `nestrs version` to confirm.");
    Ok(())
}

fn latest_crates_io_version() -> CliResult<String> {
    let output = Command::new("cargo")
        .args(["search", CRATE_NAME, "--limit", "1"])
        .output()
        .map_err(CliError::Io)?;

    if !output.status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "could not query crates.io for {CRATE_NAME} — check your network connection"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_cargo_search_version(&stdout).ok_or_else(|| {
        CliError::Anyhow(anyhow::anyhow!(
            "could not parse crates.io search output for {CRATE_NAME}"
        ))
    })
}

/// Parses `nest-rs-cli = "0.1.0"    # …` from `cargo search` output.
fn parse_cargo_search_version(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("note:") {
            continue;
        }
        let Some((name, rest)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != CRATE_NAME {
            continue;
        }
        let version = rest.trim().trim_start_matches('"').split('"').next()?;
        return Some(version.to_string());
    }
    None
}

/// Compares `major.minor.patch` semver prefixes (pre-release suffixes ignored).
fn version_cmp(left: &str, right: &str) -> Ordering {
    fn parse(version: &str) -> (u32, u32, u32) {
        let core = version.split(['-', '+']).next().unwrap_or(version);
        let mut parts = core.split('.');
        (
            parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
        )
    }
    parse(left).cmp(&parse(right))
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

    #[test]
    fn parse_cargo_search_output() {
        let stdout = concat!(
            "nest-rs-cli = \"0.1.0\"    # Scaffolding CLI for nestrs.\n",
            "note: to learn more about a package, run `cargo info <name>`\n"
        );
        assert_eq!(parse_cargo_search_version(stdout).as_deref(), Some("0.1.0"));
    }

    #[test]
    fn version_cmp_orders_semver() {
        assert_eq!(version_cmp("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(version_cmp("0.1.1", "0.1.0"), Ordering::Greater);
        assert_eq!(version_cmp("0.1.0", "0.2.0"), Ordering::Less);
        assert_eq!(version_cmp("1.0.0", "0.9.9"), Ordering::Greater);
    }
}
