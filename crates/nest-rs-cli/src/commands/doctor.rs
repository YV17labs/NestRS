use std::path::{Path, PathBuf};

use crate::context::{ENV_PREFIX_VAR, EnvPrefixSource};
use crate::error::{CliError, CliResult};

/// The floor, **derived** from the manifest rather than retyped: this crate's
/// `rust-version` is inherited from the workspace, so a bump moves the floor
/// doctor certifies against with it. Hand-typed it was a second authority on a
/// number Cargo already exports — and one that certified a toolchain the
/// workspace no longer builds on, with every floor-derived fixture in this
/// module still passing, because they read the stale constant too.
const MIN_RUST_VERSION: (u32, u32) = parse_floor(env!("CARGO_PKG_RUST_VERSION"));

/// `major.minor` of a `rust-version`, in const so a manifest that stops parsing
/// is a compile error rather than a floor of zero.
const fn parse_floor(raw: &str) -> (u32, u32) {
    let bytes = raw.as_bytes();
    let mut component = [0u32; 2];
    let mut which = 0;
    let mut i = 0;
    while i < bytes.len() && which < 2 {
        let byte = bytes[i];
        if byte == b'.' {
            which += 1;
        } else {
            assert!(byte.is_ascii_digit(), "rust-version is not major.minor");
            component[which] = component[which] * 10 + (byte - b'0') as u32;
        }
        i += 1;
    }
    (component[0], component[1])
}

pub struct DoctorOptions {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct DoctorReport {
    /// What asking for the toolchain produced, kept structural: a consumer
    /// reads the outcome rather than matching on an English sentence, and the
    /// three fields this replaced could disagree with one another.
    pub rustc: Rustc,
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

    /// Whether the toolchain meets the floor. Derived, so it cannot disagree
    /// with the outcome it summarises.
    pub fn rustc_ok(&self) -> bool {
        matches!(self.rustc, Rustc::Version { release: Some(release), .. } if release >= MIN_RUST_VERSION)
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
    // The namespace is the config's stem, never a resource word: `seaorm` and
    // `redis` are what `#[config(namespace = …)]` declares, so they are the
    // only names a project can set. `DATABASE`/`QUEUE` named neither the crate
    // nor the type that parses them, and nothing has ever read them.
    ("SEAORM", "URL", true),
    ("REDIS", "URL", true),
    ("HTTP", "HOST", false),
    ("HTTP", "PORT", false),
];

pub fn run(opts: DoctorOptions) -> CliResult<DoctorReport> {
    let start = super::resolve_start(opts.path);

    let mut report = DoctorReport {
        rustc: rustc_probe(),
        cargo_ok: which("cargo"),
        ..Default::default()
    };

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
    if !report.rustc_ok() || !report.cargo_ok || !prefix_ok {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "doctor found blocking issues — fix them before continuing"
        )));
    }

    Ok(report)
}

fn print_report(report: &DoctorReport) {
    println!("nestrs doctor");
    println!();

    // Every sentence written once, here, and every one of them names the floor
    // — a version and a verdict without the requirement is what the docs page
    // promising `rustc ≥ 1.97` described and doctor did not do.
    let (major, minor) = MIN_RUST_VERSION;
    let needs = format!("nestrs needs {major}.{minor} or newer");
    let toolchain = match &report.rustc {
        Rustc::Version { line, .. } if report.rustc_ok() => line.clone(),
        Rustc::Version {
            line,
            release: None,
        } => {
            format!("{line} — no version to read in that line; {needs}")
        }
        Rustc::Version { line, .. } => format!("{line} — {needs}"),
        Rustc::NotOnPath => format!("rustc not on PATH — {needs}"),
        Rustc::Failed(said) => format!("rustc is on PATH but failed: {said}"),
    };
    status_line("Rust toolchain", report.rustc_ok(), &toolchain);
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

/// What asking `rustc` for its version produced.
///
/// Three outcomes, kept apart all the way to the printed line. Collapsed into
/// one `None` they all rendered as `rustc not found`, so a `rustc` that *was*
/// on `PATH` and printed a diagnosis was reported as absent and its diagnosis
/// discarded — the operator was sent to fix the wrong thing.
#[derive(Debug, Default)]
pub enum Rustc {
    Version {
        line: String,
        /// `None` when the line carries no version to read — which is not the
        /// same as a compiler that is merely old, and no longer shares its
        /// sentence with one.
        release: Option<(u32, u32)>,
    },
    /// Nothing has been probed yet — a default report, never an answer.
    #[default]
    NotOnPath,
    /// On `PATH`, ran, and failed. Carries what it said.
    Failed(String),
}

pub(super) fn rustc_probe() -> Rustc {
    let output = match std::process::Command::new("rustc")
        .arg("--version")
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Rustc::NotOnPath,
        Err(e) => return Rustc::Failed(e.to_string()),
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let said = if said.is_empty() {
            format!("exited {}", output.status)
        } else {
            said
        };
        return Rustc::Failed(said);
    }
    let line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let release = rustc_release(&line);
    Rustc::Version { line, release }
}

/// The `rustc --version` line, or `None` when there is none to report. Shared
/// with `nestrs info`, which reports the toolchain without doctor's verdict and
/// so has nothing to do with *why* there isn't one.
pub(super) fn rustc_version() -> Option<String> {
    match rustc_probe() {
        Rustc::Version { line, .. } => Some(line),
        _ => None,
    }
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

/// The `(major, minor)` a `rustc --version` line reports, or `None` when the
/// line carries none to read.
///
/// Kept apart from the verdict because the two have different fixes: parsing a
/// component with `unwrap_or(0)` turned `rustc 1.x.0` into version zero, and an
/// unreadable line was then reported in the same sentence as a compiler that is
/// merely old. Both still fail closed — an unreadable version is never enough.
fn rustc_release(version_line: &str) -> Option<(u32, u32)> {
    let rest = version_line.strip_prefix("rustc ")?;
    let token = rest.split_whitespace().next()?;
    let mut parts = token.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rustc_version() {
        // Fixtures derive from the floor rather than restate it: a copied
        // literal keeps passing after `MIN_RUST_VERSION` moves without it.
        let (major, minor) = MIN_RUST_VERSION;
        assert_eq!(
            rustc_release(&format!("rustc {major}.{minor}.0 (abc 2025-01-01)")),
            Some(MIN_RUST_VERSION)
        );
        // Below the floor on the axis that always has room: decrementing the
        // minor overflowed at a `(2, 0)` floor, which is a trap for whoever
        // bumps next rather than a fixture.
        let below = format!("{}.{minor}", major.saturating_sub(1));
        assert!(!verdict(&format!("rustc {below}.0 (abc 2025-01-01)")));
        assert!(verdict(&format!(
            "rustc {major}.{minor}.0 (abc 2025-01-01)"
        )));

        // Unreadable is not old — the distinction the printed line now makes,
        // and every one of these used to parse as version zero and be reported
        // as a compiler that is merely out of date.
        assert_eq!(rustc_release("rustc 1.97"), Some((1, 97)));
        assert_eq!(rustc_release("rustc 1.97.0-nightly (abc)"), Some((1, 97)));
        assert_eq!(rustc_release("rustc 1.x.0 (abc)"), None);
        assert_eq!(rustc_release("rustc 4294967296.0.0"), None);
        assert_eq!(rustc_release("hello world"), None);
        assert_eq!(rustc_release(""), None);
        // Everything unreadable still fails closed.
        assert!(!verdict("rustc 1.x.0 (abc)"));
        assert!(!verdict("hello world"));
    }

    /// `rustc_ok` for a report whose probe returned `line` — the real path the
    /// verdict travels, rather than a comparison written twice.
    fn verdict(line: &str) -> bool {
        DoctorReport {
            rustc: Rustc::Version {
                release: rustc_release(line),
                line: line.to_owned(),
            },
            ..Default::default()
        }
        .rustc_ok()
    }

    // B9: doctor read only `std::env`, so it answered `not set` for a variable
    // the workspace's own generated `.env` defines — and then reassured the
    // reader that "none set" was fine for their DB-backed app.
    #[test]
    fn a_cascade_file_counts_as_set() {
        assert!(file_defines(
            "NESTRS_SEAORM__URL=postgres://x",
            "NESTRS_SEAORM__URL"
        ));
        assert!(file_defines(
            "export NESTRS_SEAORM__URL=postgres://x",
            "NESTRS_SEAORM__URL"
        ));
        assert!(file_defines(
            "# a comment\nNESTRS_REDIS__URL=redis://x\n",
            "NESTRS_REDIS__URL"
        ));
    }

    #[test]
    fn a_commented_or_empty_assignment_does_not_count() {
        assert!(!file_defines(
            "# NESTRS_SEAORM__URL=postgres://x",
            "NESTRS_SEAORM__URL"
        ));
        assert!(!file_defines("NESTRS_SEAORM__URL=", "NESTRS_SEAORM__URL"));
        assert!(!file_defines(
            "NESTRS_SEAORM__URL=   ",
            "NESTRS_SEAORM__URL"
        ));
        // A different key with a matching prefix must not answer for it.
        assert!(!file_defines(
            "NESTRS_SEAORM__URL_EXTRA=x",
            "NESTRS_SEAORM__URL"
        ));
    }

    #[test]
    fn the_cascade_is_consulted_from_the_starting_directory() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env"), "NESTRS_SEAORM__URL=postgres://x\n").expect("write");
        let cascade = cascade_text(&dir, "NESTRS");
        assert!(env_present(&cascade, "NESTRS_SEAORM__URL"));
        assert!(!env_present(&cascade, "NESTRS_REDIS__URL"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A project that renamed its variables must be answered in its own names.
    /// Reporting `NESTRS_SEAORM__URL: not set` there is worse than silence:
    /// it sends the reader to add a key the app will never read.
    #[test]
    fn a_custom_prefix_project_is_answered_in_its_own_variable_names() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-acme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env"), "ACME_SEAORM__URL=postgres://x\n").expect("write");
        let cascade = cascade_text(&dir, "ACME");
        assert!(env_present(&cascade, "ACME_SEAORM__URL"));
        assert!(
            !env_present(&cascade, "NESTRS_SEAORM__URL"),
            "the default name must not answer for a renamed project",
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
