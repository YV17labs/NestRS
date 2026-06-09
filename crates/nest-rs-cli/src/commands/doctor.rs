use std::path::PathBuf;

use crate::error::{CliError, CliResult};

const MIN_RUST_VERSION: (u32, u32) = (1, 95);

pub struct DoctorOptions {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub rustc_ok: bool,
    pub rustc_version: Option<String>,
    pub cargo_ok: bool,
    pub in_nestrs_workspace: bool,
    pub env_database: Option<bool>,
    pub env_queue: Option<bool>,
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

    if let Ok(Some(_)) = crate::context::NestrsWorkspace::discover(&start) {
        report.in_nestrs_workspace = true;
    }

    report.env_database = env_present("NESTRS_DATABASE__URL");
    report.env_queue = env_present("NESTRS_QUEUE__URL");
    report.env_http_host = std::env::var("NESTRS_HTTP__HOST").is_ok();
    report.env_http_port = std::env::var("NESTRS_HTTP__PORT").is_ok();

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
        report
            .rustc_version
            .as_deref()
            .unwrap_or("rustc not found"),
    );
    status_line("cargo", report.cargo_ok, if report.cargo_ok { "ok" } else { "not found" });

    if report.in_nestrs_workspace {
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
    if report.env_database != Some(true)
        && report.env_queue != Some(true)
        && !report.env_http_host
        && !report.env_http_port
    {
        println!("  (none set — fine for bare HTTP apps on defaults)");
    }
    println!();
}

fn status_line(label: &str, ok: bool, detail: &str) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("  [{mark}] {label}: {detail}");
}

fn print_env_hint(name: &str, present: Option<bool>) {
    match present {
        Some(true) => println!("  {name}: set"),
        Some(false) => println!("  {name}: not set"),
        None => {}
    }
}

fn env_present(name: &str) -> Option<bool> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(true),
        Ok(_) => Some(false),
        Err(_) => Some(false),
    }
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
        assert!(version_at_least("rustc 1.95.0 (abc 2025-01-01)", (1, 95)));
        assert!(!version_at_least("rustc 1.94.0 (abc 2025-01-01)", (1, 95)));
    }
}
