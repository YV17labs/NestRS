use std::path::{Path, PathBuf};

use crate::error::{CliError, CliResult};

const MIN_RUST_VERSION: (u32, u32) = (1, 96);

pub struct DoctorOptions {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub rustc_ok: bool,
    pub rustc_version: Option<String>,
    pub cargo_ok: bool,
    pub in_nestrs_workspace: bool,
    /// Set when workspace detection itself failed (e.g. a malformed manifest),
    /// as distinct from a clean "not a workspace" result.
    pub workspace_error: Option<String>,
    pub env_database: bool,
    pub env_queue: bool,
    pub env_http_host: bool,
    pub env_http_port: bool,
}

pub fn run(opts: DoctorOptions) -> CliResult<DoctorReport> {
    let start = opts
        .path
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let mut report = DoctorReport::default();

    report.rustc_version = rustc_version();
    report.rustc_ok = report
        .rustc_version
        .as_ref()
        .is_some_and(|v| version_at_least(v, MIN_RUST_VERSION));
    report.cargo_ok = which("cargo");

    match crate::context::NestrsWorkspace::discover(&start) {
        Ok(Some(_)) => report.in_nestrs_workspace = true,
        Ok(None) => {}
        Err(e) => report.workspace_error = Some(e.to_string()),
    }

    // One cascade read for all four, rather than up to four files per variable.
    let cascade = cascade_text(&start);
    report.env_database = env_present(&cascade, "NESTRS_DATABASE__URL");
    report.env_queue = env_present(&cascade, "NESTRS_QUEUE__URL");
    report.env_http_host = env_present(&cascade, "NESTRS_HTTP__HOST");
    report.env_http_port = env_present(&cascade, "NESTRS_HTTP__PORT");

    print_report(&report);

    if !report.rustc_ok || !report.cargo_ok {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "doctor found blocking issues — fix them before continuing"
        )));
    }

    Ok(report)
}

fn print_report(report: &DoctorReport) {
    println!("nestrs doctor");
    println!();

    status_line(
        "Rust toolchain",
        report.rustc_ok,
        report.rustc_version.as_deref().unwrap_or("rustc not found"),
    );
    status_line(
        "cargo",
        report.cargo_ok,
        if report.cargo_ok { "ok" } else { "not found" },
    );

    if let Some(err) = &report.workspace_error {
        println!("  nestrs workspace: detection failed: {err}");
    } else if report.in_nestrs_workspace {
        println!("  nestrs workspace: yes");
    } else {
        println!("  nestrs workspace: no (standalone project or outside a clone)");
    }

    println!();
    println!("Environment (optional — only needed for DB/Redis apps):");
    print_env_hint("NESTRS_DATABASE__URL", report.env_database);
    print_env_hint("NESTRS_QUEUE__URL", report.env_queue);
    if report.env_http_host {
        println!("  NESTRS_HTTP__HOST: set");
    }
    if report.env_http_port {
        println!("  NESTRS_HTTP__PORT: set");
    }
    if !report.env_database && !report.env_queue && !report.env_http_host && !report.env_http_port {
        println!("  (none set — fine for bare HTTP apps on defaults)");
    }
    println!();
}

fn status_line(label: &str, ok: bool, detail: &str) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("  [{mark}] {label}: {detail}");
}

fn print_env_hint(name: &str, present: bool) {
    println!("  {name}: {}", if present { "set" } else { "not set" });
}

/// Whether an app started here would resolve `name` — the real process
/// environment **or** the `.env` cascade.
///
/// Reading only `std::env` is the exact mistake `/database/migrations/` warns
/// tool authors against, and it made doctor report `not set` for a variable the
/// workspace's own generated `.env` defines — then reassure the reader that
/// "none set" was fine.
///
/// The cascade is re-read here rather than borrowed from `nest-rs-config`: the
/// CLI deliberately depends on no framework crate, so that `cargo install
/// nest-rs-cli` stays independent of the version a project pins. Only presence
/// is answered, so this stays a scan for the key, not a second value parser.
fn env_present(cascade: &str, name: &str) -> bool {
    matches!(std::env::var(name), Ok(v) if !v.trim().is_empty()) || file_defines(cascade, name)
}

/// Every cascade file rooted at `dir`, concatenated. Mirrors
/// `nest_rs_config::dotenv`'s file set — including skipping `.env.local` under
/// `NESTRS_ENV=test`, so doctor answers what an app would actually resolve.
/// Precedence does not matter here: the question is presence, not value.
fn cascade_text(dir: &Path) -> String {
    let env = std::env::var("NESTRS_ENV").unwrap_or_else(|_| "development".to_owned());
    let env = env.trim().to_owned();
    let mut files = vec![format!(".env.{env}.local")];
    if env != "test" {
        files.push(".env.local".to_owned());
    }
    files.push(format!(".env.{env}"));
    files.push(".env".to_owned());
    files
        .iter()
        .filter_map(|file| std::fs::read_to_string(dir.join(file)).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

/// One file's answer, split out so the line grammar (`export` prefix,
/// comments, `KEY=` counting as unset) is unit-testable.
fn file_defines(contents: &str, name: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return false;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        line.split_once('=')
            .is_some_and(|(key, value)| key.trim() == name && !value.trim().is_empty())
    })
}

fn rustc_version() -> Option<String> {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn which(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn version_at_least(version_line: &str, min: (u32, u32)) -> bool {
    let Some(rest) = version_line.strip_prefix("rustc ") else {
        return false;
    };
    let version_token = rest.split_whitespace().next().unwrap_or("");
    let mut parts = version_token.split('.');
    let major: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let minor: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    (major, minor) >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rustc_version() {
        assert!(version_at_least("rustc 1.96.0 (abc 2025-01-01)", (1, 96)));
        assert!(!version_at_least("rustc 1.95.0 (abc 2025-01-01)", (1, 96)));
    }

    // B9: doctor read only `std::env`, so it answered `not set` for a variable
    // the workspace's own generated `.env` defines — and then reassured the
    // reader that "none set" was fine for their DB-backed app.
    #[test]
    fn a_cascade_file_counts_as_set() {
        assert!(file_defines(
            "NESTRS_DATABASE__URL=postgres://x",
            "NESTRS_DATABASE__URL"
        ));
        assert!(file_defines(
            "export NESTRS_DATABASE__URL=postgres://x",
            "NESTRS_DATABASE__URL"
        ));
        assert!(file_defines(
            "# a comment\nNESTRS_QUEUE__URL=redis://x\n",
            "NESTRS_QUEUE__URL"
        ));
    }

    #[test]
    fn a_commented_or_empty_assignment_does_not_count() {
        assert!(!file_defines(
            "# NESTRS_DATABASE__URL=postgres://x",
            "NESTRS_DATABASE__URL"
        ));
        assert!(!file_defines(
            "NESTRS_DATABASE__URL=",
            "NESTRS_DATABASE__URL"
        ));
        assert!(!file_defines(
            "NESTRS_DATABASE__URL=   ",
            "NESTRS_DATABASE__URL"
        ));
        // A different key with a matching prefix must not answer for it.
        assert!(!file_defines(
            "NESTRS_DATABASE__URL_EXTRA=x",
            "NESTRS_DATABASE__URL"
        ));
    }

    #[test]
    fn the_cascade_is_consulted_from_the_starting_directory() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env"), "NESTRS_DATABASE__URL=postgres://x\n").expect("write");
        let cascade = cascade_text(&dir);
        assert!(env_present(&cascade, "NESTRS_DATABASE__URL"));
        assert!(!env_present(&cascade, "NESTRS_QUEUE__URL"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
