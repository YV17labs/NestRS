use std::path::{Path, PathBuf};

use crate::context::{ENV_PREFIX_VAR, EnvPrefixSource};
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
    /// Where the prefix came from — reported rather than just resolved,
    /// because "this shell names none" and "this shell names ACME" produce the
    /// same variable list only by accident, and the operator needs to know
    /// which of the two they are looking at.
    pub env_prefix_source: EnvPrefixSource,
    /// Each optional variable doctor looked for, resolved name and all, in
    /// report order. Names are stored rather than rebuilt at print time: the
    /// name reported is then the name probed, by construction.
    pub env_vars: Vec<EnvVar>,
}

impl DoctorReport {
    /// The prefix every name below is built from — derived, never stored, so
    /// the two cannot disagree.
    pub fn env_prefix(&self) -> &str {
        self.env_prefix_source.prefix()
    }
}

#[derive(Debug)]
pub struct EnvVar {
    pub name: String,
    pub present: bool,
    /// Listed even when unset — the two backends an app is most likely to be
    /// missing. The rest are only worth a line when they *are* set.
    always_reported: bool,
}

/// The optional variables doctor answers for, as `(namespace, key, always
/// reported)`.
const CHECKED: &[(&str, &str, bool)] = &[
    ("DATABASE", "URL", true),
    ("QUEUE", "URL", true),
    ("HTTP", "HOST", false),
    ("HTTP", "PORT", false),
];

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

    // Read from *this* environment, which is the same source the app reads —
    // no project file to disagree with. The layout is irrelevant here: a
    // workspace and a standalone crate resolve the prefix identically.
    report.env_prefix_source = EnvPrefixSource::detect();

    // One cascade read for all four, rather than up to four files per variable.
    let cascade = cascade_text(&start, report.env_prefix());
    report.env_vars = CHECKED
        .iter()
        .map(|&(namespace, key, always_reported)| {
            let name = crate::context::var_name(report.env_prefix(), namespace, key);
            EnvVar {
                present: env_present(&cascade, &name),
                name,
                always_reported,
            }
        })
        .collect();

    print_report(&report);

    // An unusable prefix blocks like a missing toolchain does: every app in
    // this environment aborts on the first name it builds.
    let prefix_ok = !matches!(report.env_prefix_source, EnvPrefixSource::Invalid(_));
    if !report.rustc_ok || !report.cargo_ok || !prefix_ok {
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
    // Named even on the default, so the answers below are unambiguous: a reader
    // seeing `not set` can tell a missing value from a prefix mismatch. The
    // source comes with it, because doctor answers for the shell it runs in —
    // a project whose deployment renames its variables looks untouched from a
    // terminal that does not.
    match &report.env_prefix_source {
        EnvPrefixSource::Environment(prefix) => {
            println!("  env prefix: {prefix} (from {ENV_PREFIX_VAR})");
        }
        EnvPrefixSource::Unset => {
            println!(
                "  env prefix: {} (default — {ENV_PREFIX_VAR} is not set here, so the names \
                 below are this shell's view, not your deployment's)",
                report.env_prefix(),
            );
        }
        EnvPrefixSource::Invalid(reason) => {
            println!("  env prefix: {ENV_PREFIX_VAR} is unusable — {reason}");
            println!("              an app started with it set aborts at boot.");
        }
    }
    for var in &report.env_vars {
        if var.present {
            println!("  {}: set", var.name);
        } else if var.always_reported {
            println!("  {}: not set", var.name);
        }
    }
    if report.env_vars.iter().all(|var| !var.present) {
        println!("  (none set — fine for bare HTTP apps on defaults)");
    }
    println!();
}

fn status_line(label: &str, ok: bool, detail: &str) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("  [{mark}] {label}: {detail}");
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
/// `<PREFIX>_ENV=test`, so doctor answers what an app would actually resolve.
/// Precedence does not matter here: the question is presence, not value.
fn cascade_text(dir: &Path, env_prefix: &str) -> String {
    let env =
        std::env::var(format!("{env_prefix}_ENV")).unwrap_or_else(|_| "development".to_owned());
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

/// The `rustc --version` line, or `None` when it is not on `PATH`. Shared with
/// `nestrs info`, which reports the same toolchain without doctor's verdict.
pub(super) fn rustc_version() -> Option<String> {
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
        let cascade = cascade_text(&dir, "NESTRS");
        assert!(env_present(&cascade, "NESTRS_DATABASE__URL"));
        assert!(!env_present(&cascade, "NESTRS_QUEUE__URL"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A project that renamed its variables must be answered in its own names.
    /// Reporting `NESTRS_DATABASE__URL: not set` there is worse than silence:
    /// it sends the reader to add a key the app will never read.
    #[test]
    fn a_custom_prefix_project_is_answered_in_its_own_variable_names() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-acme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env"), "ACME_DATABASE__URL=postgres://x\n").expect("write");
        let cascade = cascade_text(&dir, "ACME");
        assert!(env_present(&cascade, "ACME_DATABASE__URL"));
        assert!(
            !env_present(&cascade, "NESTRS_DATABASE__URL"),
            "the default name must not answer for a renamed project",
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
