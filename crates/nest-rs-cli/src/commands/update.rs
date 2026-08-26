use std::cmp::Ordering;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};

const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

pub struct UpdateOptions {
    /// Reinstall from `crates/nest-rs-cli` in the nestrs monorepo instead of crates.io.
    pub from_path: bool,
    /// Workspace root when using `--workspace` (default: auto-discover).
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
    cmd.arg("--locked").arg(CRATE_NAME);

    cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let status = cmd.status().map_err(CliError::Io)?;
    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo install failed — try manually: cargo install --locked {CRATE_NAME}"
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
        // `cargo search` fails for reasons that are not the network — an
        // unauthenticated private registry, a `[source]` replacement, a rate
        // limit — and it says which on stderr. Swallowing it sends the reader
        // to fix their connection while the registry is what refused them.
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.trim();
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "could not query crates.io for {CRATE_NAME}{}",
            if said.is_empty() {
                " — check your network connection".to_owned()
            } else {
                format!(": {said}")
            }
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
        // Parsed at the edge, so an unreadable line is `None` and the caller
        // says so. Waved through, it reached `version_cmp`'s `unwrap_or(0)` as
        // `0.0.0` — and the CLI then reported itself *newer* than crates.io and
        // exited 0, which is the shape `doctor` already records as a defect it
        // fixed for `rustc`.
        return is_semver_prefixed(version).then(|| version.to_string());
    }
    None
}

/// The `major.minor.patch` prefix, or `None` when any of the three is not a
/// number — `1.2.3` and `1.2.3-rc.1` read, `1.x.0` and `5.1` do not.
///
/// One parser, because "readable version" is one fact: the rejection below and
/// the comparison above it disagreeing is how `1.x.0` reached a comparison as
/// `0.0.0` in the first place.
fn semver_prefix(version: &str) -> Option<(u32, u32, u32)> {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = core.split('.');
    let mut component = || parts.next().and_then(|p| p.parse::<u32>().ok());
    Some((component()?, component()?, component()?))
}

/// Whether the three leading components are numeric — `1.2.3`, `1.2.3-rc.1`.
fn is_semver_prefixed(version: &str) -> bool {
    semver_prefix(version).is_some()
}

/// Compares `major.minor.patch` semver prefixes (pre-release suffixes ignored).
///
/// An unreadable version sorts as `0.0.0`, which is only ever reached by a
/// caller that did not filter through [`is_semver_prefixed`] first.
fn version_cmp(left: &str, right: &str) -> Ordering {
    semver_prefix(left)
        .unwrap_or_default()
        .cmp(&semver_prefix(right).unwrap_or_default())
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

    /// An unreadable version is `None`, not `0.0.0`: waved through, it made the
    /// CLI announce itself newer than the registry and exit 0.
    #[test]
    fn an_unparseable_version_is_rejected_rather_than_read_as_zero() {
        let stdout = "nest-rs-cli = \"1.x.0\"    # Scaffolding CLI.\n";
        assert_eq!(parse_cargo_search_version(stdout), None);
        assert!(is_semver_prefixed("5.1.0"));
        assert!(is_semver_prefixed("5.1.0-rc.1"));
        assert!(!is_semver_prefixed("5.1"));
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
