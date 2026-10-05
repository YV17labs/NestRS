use std::ffi::OsString;
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

pub(crate) struct DoctorOptions {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub(crate) struct DoctorReport {
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
    /// What the `.env` cascade names that the loader refuses as a whole — see
    /// [`cascade_refusals`].
    pub cascade_refusals: Vec<String>,
}

impl DoctorReport {
    /// The prefix every name below is built from — derived, never stored, so
    /// the two cannot disagree.
    pub(crate) fn env_prefix(&self) -> &str {
        self.env_prefix_source.prefix()
    }

    /// Whether the toolchain meets the floor. Derived, so it cannot disagree
    /// with the outcome it summarises.
    pub(crate) fn rustc_ok(&self) -> bool {
        matches!(self.rustc, Rustc::Version { release: Some(release), .. } if release >= MIN_RUST_VERSION)
    }
}

#[derive(Debug)]
pub(crate) struct EnvVar {
    pub name: String,
    pub resolution: Resolution,
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

pub(crate) fn run(opts: DoctorOptions) -> CliResult<DoctorReport> {
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
    // no project file to disagree with.
    report.env_prefix_source = EnvPrefixSource::detect();

    // One cascade read for all four, rather than up to four files per variable.
    let cascade = cascade_text(&start, report.env_prefix(), process_env);
    report.cascade_refusals = cascade_refusals(process_env, &cascade, report.env_prefix());
    report.env_vars = CHECKED
        .iter()
        .map(|&(namespace, key, always_reported)| {
            let name = crate::context::var_name(report.env_prefix(), namespace, key);
            EnvVar {
                resolution: resolve_variable(process_env, &cascade, &name, &start),
                name,
                always_reported,
            }
        })
        .collect();

    print_report(&report);

    // An unusable prefix blocks like a missing toolchain does: every app in
    // this environment aborts on the first name it builds. So does a variable
    // the loader would refuse — a `_FILE` it cannot read, a value given twice:
    // every app that reads it aborts its boot on it — and a cascade the loader
    // refuses whole, which aborts every app at its first config read.
    let prefix_ok = !matches!(report.env_prefix_source, EnvPrefixSource::Invalid(_));
    let variables_ok = report
        .env_vars
        .iter()
        .all(|var| !matches!(var.resolution, Resolution::Refused(_)));
    let cascade_ok = report.cascade_refusals.is_empty();
    if !report.rustc_ok() || !report.cargo_ok || !prefix_ok || !variables_ok || !cascade_ok {
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
        println!("  nestrs workspace: no (outside a nestrs workspace)");
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
    for refusal in &report.cascade_refusals {
        println!("  {refusal}");
    }
    for var in &report.env_vars {
        match &var.resolution {
            Resolution::Set => println!("  {}: set", var.name),
            Resolution::Unset if var.always_reported => println!("  {}: not set", var.name),
            Resolution::Unset => {}
            Resolution::Refused(reason) => {
                println!(
                    "  {}: an app started here fails its boot — {reason}",
                    var.name
                );
            }
        }
    }
    if report.cascade_refusals.is_empty()
        && report
            .env_vars
            .iter()
            .all(|var| var.resolution == Resolution::Unset)
    {
        println!("  (none set — fine for bare HTTP apps on defaults)");
    }
    println!();
}

fn status_line(label: &str, ok: bool, detail: &str) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("  [{mark}] {label}: {detail}");
}

/// What an app started here makes of one variable, as the loader's
/// `ConfigService::setting` answers it: a value, nothing, or a boot error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A value the app reads.
    Set,
    /// Nothing — the variable is unset, empty, or names a file holding only
    /// line breaks.
    Unset,
    /// A boot error, and why — naming variables, never a value or what a file
    /// holds.
    Refused(String),
}

/// The process environment, as [`resolve_variable`] and [`cascade_text`] read
/// it — the one place doctor consults the shell it runs in, so every helper
/// below takes its environment as an argument and a test hands it one.
#[expect(
    clippy::disallowed_methods,
    reason = "doctor reports what the process environment holds, without linking the loader"
)]
fn process_env(var: &str) -> Option<OsString> {
    std::env::var_os(var)
}

/// The most a `<NAME>_FILE` is read for — the loader's `read_material` bound.
const MAX_MATERIAL_BYTES: u64 = 1024 * 1024;

/// What an app started in `dir` makes of `name` — the real process environment
/// (`real`) **or** the `.env` cascade (`cascade`), inline or through the file
/// `<NAME>_FILE` names — answered as the loader answers it.
///
/// **The deployment chooses the spelling**: when the process environment holds
/// either, empty included, both are read from the process alone and the
/// cascade is shadowed; a value that is not UTF-8 there is unset, as the loader
/// reads it. An empty value is unset in either tier, and nothing is trimmed
/// that the loader keeps. Both spellings set is refused; a `_FILE` path has its
/// surrounding whitespace trimmed and must name a regular file of at most a
/// mebibyte holding UTF-8 text — a file holding only line breaks is unset. **A
/// relative path is opened from `dir`**, the directory the app is started in,
/// since the loader opens it from its own working directory: opened from
/// doctor's instead, `nestrs doctor -p <dir>` failed a correct project from
/// outside it and passed one whose file sat beside the caller.
///
/// **A mirror, not a borrow, and held to its original by execution.** The CLI
/// depends on no framework crate, so that `cargo install nest-rs-cli` stays
/// independent of the version a project pins; the conformance suite runs this
/// function beside the loader over every shape a deployment can give, which is
/// what keeps the two from drifting. It answered `set` for a `_FILE` naming an
/// empty or missing file, for a value given twice, and for a shell value that
/// is not UTF-8 — four answers the loader contradicts. Reading only `std::env`
/// is the mistake `/database/migrations/` warns tool authors against: it made
/// doctor report `not set` for a variable the workspace's own `.env` defines.
pub fn resolve_variable(
    real: impl Fn(&str) -> Option<OsString>,
    cascade: &str,
    name: &str,
    dir: &Path,
) -> Resolution {
    let file_name = format!("{name}_FILE");
    let deployment = real(name).is_some() || real(&file_name).is_some();
    let read = |var: &str| {
        let value = if deployment {
            real(var).and_then(|value| value.into_string().ok())
        } else {
            cascade_value(cascade, var)
        };
        value.filter(|value| !value.is_empty())
    };
    match (read(name), read(&file_name)) {
        (None, None) => Resolution::Unset,
        (Some(_), None) => Resolution::Set,
        (Some(_), Some(_)) => Resolution::Refused(format!(
            "{name} is set together with {file_name}: give the value once, inline or as a path"
        )),
        (None, Some(path)) => file_resolution(
            &file_name,
            &dir.join(path.trim_matches(|c: char| c.is_ascii_whitespace())),
        ),
    }
}

/// The file `file_name` names, read as the loader reads it.
fn file_resolution(file_name: &str, path: &Path) -> Resolution {
    let unreadable = |why: &str| Resolution::Refused(format!("{file_name} names {why}"));
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return unreadable("a file that cannot be read"),
    };
    if !metadata.is_file() {
        return unreadable("something other than a regular file");
    }
    if metadata.len() > MAX_MATERIAL_BYTES {
        return unreadable("a file larger than a mebibyte, which no configuration value is");
    }
    let Ok(bytes) = std::fs::read(path) else {
        return unreadable("a file that cannot be read");
    };
    match String::from_utf8(bytes) {
        Err(_) => unreadable("a file that is not UTF-8 text"),
        Ok(text) if text.trim_end_matches(['\n', '\r']).is_empty() => Resolution::Unset,
        Ok(_) => Resolution::Set,
    }
}

/// What the `.env` cascade (`cascade`) names that the loader refuses as a
/// whole, every app started here aborting at its first config read: the two
/// variables that choose the cascade, written into it.
///
/// The prefix (`NESTRS_ENV_PREFIX`) is refused unless it restates the one
/// resolved from the process (`prefix`), and the environment selector
/// (`<PREFIX>_ENV`) unless the process (`real`) carries the same value — a
/// value inside the cascade arrives after the files were chosen, and the
/// selector arms development-only affordances, which no committed file may do.
/// That is `nest_rs_config`'s `cascade_map`, mirrored like
/// [`resolve_variable`] and held to it by the same conformance join. Each
/// sentence names the variable, never its value.
pub fn cascade_refusals(
    real: impl Fn(&str) -> Option<OsString>,
    cascade: &str,
    prefix: &str,
) -> Vec<String> {
    let mut refused = Vec::new();
    if cascade_value(cascade, ENV_PREFIX_VAR).is_some_and(|declared| declared != prefix) {
        refused.push(format!(
            "{ENV_PREFIX_VAR} in the .env cascade: every app started here aborts at its first \
             config read — it names another prefix than `{prefix}`, the one the process \
             resolved, and the prefix chooses which cascade is read, so a value inside one \
             arrives too late: set it on the process (your container, your shell, the Justfile)"
        ));
    }
    let selector = format!("{prefix}_ENV");
    if let Some(declared) = cascade_value(cascade, &selector) {
        let process = real(&selector)
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.is_empty());
        if process.as_deref() != Some(declared.as_str()) {
            refused.push(format!(
                "{selector} in the .env cascade: every app started here aborts at its first \
                 config read — the process does not carry the same value, and the variable \
                 chooses which `.env` files are read, so a value inside one arrives too late: \
                 set it on the process (`nestrs run dev` does) and remove it from the file"
            ));
        }
    }
    refused
}

/// Every cascade file rooted at `dir`, concatenated. Mirrors
/// `nest_rs_config::dotenv`'s file set — including skipping `.env.local` under
/// `<PREFIX>_ENV=test`, so doctor answers what an app would actually resolve.
/// Most specific first, as the loader merges them: the first assignment of a key wins.
///
/// `real` is the process environment the selector is read from: a parameter so
/// the answer is a function of what it is handed, and a test never inherits the
/// `<PREFIX>_ENV` of the shell that runs it.
fn cascade_text(dir: &Path, env_prefix: &str, real: impl Fn(&str) -> Option<OsString>) -> String {
    let declared = real(&format!("{env_prefix}_ENV")).and_then(|value| value.into_string().ok());
    let env = cascade_environment(declared.as_deref());
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

/// The environment whose files the loader reads for a `<PREFIX>_ENV` value —
/// `nest_rs_config::Environment`'s classification, mirrored because the CLI
/// links no framework crate. Its aliases and its fallback are the loader's:
/// `prod` reads `.env.production`, and empty, unset or unrecognised read the
/// development files. Taking the raw value as a file name instead sent doctor
/// to `.env.prod` and `.env.` — files no app ever reads.
fn cascade_environment(declared: Option<&str>) -> &'static str {
    match declared.map(str::trim) {
        Some("production" | "prod") => "production",
        Some("staging" | "stage") => "staging",
        Some("test") => "test",
        _ => "development",
    }
}

/// The cascade's value for `name`, split out so the line grammar (`export`
/// prefix, comments, quotes) is unit-testable. The first assignment wins, as
/// the loader's set-if-absent merge does — `cascade_text` concatenates the files
/// most specific first — so an empty assignment in a more specific file unsets
/// the key for every file after it. A quoted value is unquoted as the loader
/// unquotes it, since a `_FILE` path read from here is opened.
fn cascade_value(contents: &str, name: &str) -> Option<String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            line.split_once('=')
                .filter(|(key, _)| key.trim() == name)
                .map(|(_, value)| unquote(value.trim()))
        })
        .next()
}

/// A `.env` value as the loader's `parse_value` reads it: single quotes taken
/// literally, double quotes with `\n`, `\t`, `\r`, `\\` and `\"` expanded
/// and any other escape kept verbatim, anything else as written.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    let quoted = bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0];
    if !quoted {
        return value.to_owned();
    }
    let inner = &value[1..value.len() - 1];
    if bytes[0] == b'\'' {
        return inner.to_owned();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// What asking `rustc` for its version produced.
///
/// Three outcomes, kept apart all the way to the printed line. Collapsed into
/// one `None` they all rendered as `rustc not found`, so a `rustc` that *was*
/// on `PATH` and printed a diagnosis was reported as absent and its diagnosis
/// discarded — the operator was sent to fix the wrong thing.
#[derive(Debug, Default)]
pub(crate) enum Rustc {
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
    /// Whether the cascade text alone sets `name`.
    fn file_defines(contents: &str, name: &str) -> bool {
        cascade_value(contents, name).is_some_and(|value| !value.is_empty())
    }

    /// Whether an app reads a value for `name`.
    fn present(real: impl Fn(&str) -> Option<OsString>, cascade: &str, name: &str) -> bool {
        resolve_variable(real, cascade, name, Path::new("/")) == Resolution::Set
    }

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

    /// Hermetic: the process environment is handed in empty, so a shell that
    /// exports `<PREFIX>_REDIS__URL` — the one a developer running this suite is
    /// most likely to have — cannot answer for the cascade.
    #[test]
    fn the_cascade_is_consulted_from_the_starting_directory() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env"), "NESTRS_SEAORM__URL=postgres://x\n").expect("write");
        let cascade = cascade_text(&dir, "NESTRS", real(&[]));
        assert!(present(real(&[]), &cascade, "NESTRS_SEAORM__URL"));
        assert!(!present(real(&[]), &cascade, "NESTRS_REDIS__URL"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The selector is the handed environment's, classified as the loader
    /// classifies it: an alias reads the canonical files, and `test` skips
    /// `.env.local`.
    #[test]
    fn the_cascade_files_follow_the_declared_environment() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env.production"), "NESTRS_REDIS__URL=redis://x\n")
            .expect("write");
        std::fs::write(dir.join(".env.local"), "NESTRS_SEAORM__URL=postgres://x\n").expect("write");

        let prod = cascade_text(&dir, "NESTRS", real(&[("NESTRS_ENV", "prod")]));
        assert!(
            present(real(&[]), &prod, "NESTRS_REDIS__URL"),
            "prod reads .env.production"
        );
        let test = cascade_text(&dir, "NESTRS", real(&[("NESTRS_ENV", "test")]));
        assert!(
            !present(real(&[]), &test, "NESTRS_SEAORM__URL"),
            "test skips .env.local"
        );
        let unset = cascade_text(&dir, "NESTRS", real(&[]));
        assert!(present(real(&[]), &unset, "NESTRS_SEAORM__URL"));
        assert!(!present(real(&[]), &unset, "NESTRS_REDIS__URL"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_environment_is_classified_as_the_loader_classifies_it() {
        assert_eq!(cascade_environment(Some("prod")), "production");
        assert_eq!(cascade_environment(Some(" production ")), "production");
        assert_eq!(cascade_environment(Some("stage")), "staging");
        assert_eq!(cascade_environment(Some("test")), "test");
        assert_eq!(cascade_environment(Some("dev")), "development");
        assert_eq!(cascade_environment(Some("")), "development");
        assert_eq!(cascade_environment(Some("producton")), "development");
        assert_eq!(cascade_environment(None), "development");
    }

    /// A variable given as a file is set: the loader reads `<NAME>_FILE` as the
    /// same variable, so doctor must not report it missing — when the file
    /// holds a value, as the loader reads it.
    #[test]
    fn a_variable_given_as_a_file_is_reported_set() {
        let dir = scratch("file-set");
        let secret = dir.join("redis-url");
        std::fs::write(&secret, "redis://x\n").expect("write");
        let cascade = format!("NESTRS_REDIS__URL_FILE=\"{}\"\n", secret.display());
        assert!(present(real(&[]), &cascade, "NESTRS_REDIS__URL"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The four answers the audit found doctor giving against the loader: a
    /// `_FILE` naming an empty file is unset, one naming a missing file is a
    /// boot error, a value given twice is a boot error, and a shell value that
    /// is not UTF-8 is unset.
    #[test]
    fn a_variable_is_answered_as_the_loader_reads_it() {
        let dir = scratch("loader");
        let empty = dir.join("empty");
        std::fs::write(&empty, "\n\r\n").expect("write");
        let missing = dir.join("missing");
        let name = "NESTRS_SEAORM__URL";
        let file = "NESTRS_SEAORM__URL_FILE";
        let as_file = |path: &Path| real(&[(file, path.to_str().expect("a UTF-8 path"))]);

        assert_eq!(
            resolve_variable(as_file(&empty), "", name, &dir),
            Resolution::Unset
        );
        let Resolution::Refused(reason) = resolve_variable(as_file(&missing), "", name, &dir)
        else {
            panic!("a missing file fails the boot");
        };
        assert!(
            reason.contains(file) && !reason.contains("missing"),
            "{reason}"
        );
        assert!(matches!(
            resolve_variable(
                real(&[(name, "postgres://x"), (file, "/run/x")]),
                "",
                name,
                &dir
            ),
            Resolution::Refused(reason) if reason.contains(name) && reason.contains(file)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let not_utf8 = |var: &str| (var == name).then(|| OsString::from_vec(vec![0xff, 0xfe]));
            assert_eq!(
                resolve_variable(not_utf8, "NESTRS_SEAORM__URL=postgres://x\n", name, &dir),
                Resolution::Unset,
                "the shell value shadows the cascade and reads as unset",
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A scratch directory unique to this process and `tag`.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A process environment holding `vars`, for [`resolve_variable`].
    fn real(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| {
            vars.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    /// The deployment chooses the spelling: an empty variable in the shell
    /// shadows both spellings in the cascade, as the loader reads it.
    #[test]
    fn an_empty_shell_variable_shadows_the_cascade_file_spelling() {
        let cascade = "NESTRS_SEAORM__URL_FILE=/nonexistent\n";
        assert_eq!(
            resolve_variable(
                real(&[("NESTRS_SEAORM__URL", "")]),
                cascade,
                "NESTRS_SEAORM__URL",
                Path::new("/")
            ),
            Resolution::Unset,
        );
        assert!(matches!(
            resolve_variable(real(&[]), cascade, "NESTRS_SEAORM__URL", Path::new("/")),
            Resolution::Refused(_)
        ));
    }

    /// Emptiness is the loader's: a blank shell value is set, since the loader
    /// trims nothing, and an empty one is not.
    #[test]
    fn a_shell_value_is_judged_empty_exactly_as_the_loader_judges_it() {
        assert!(present(
            real(&[("NESTRS_REDIS__URL", " ")]),
            "",
            "NESTRS_REDIS__URL"
        ));
        assert!(!present(
            real(&[("NESTRS_REDIS__URL", "")]),
            "",
            "NESTRS_REDIS__URL"
        ));
        assert!(matches!(
            resolve_variable(
                real(&[("NESTRS_REDIS__URL_FILE", "/run/secrets/url")]),
                "",
                "NESTRS_REDIS__URL",
                Path::new("/")
            ),
            Resolution::Refused(_)
        ));
    }

    /// The cascade merges set-if-absent, most specific file first: an empty
    /// assignment there unsets the key for the files after it, and a quoted
    /// empty value is empty.
    #[test]
    fn the_first_cascade_assignment_wins_even_when_empty() {
        assert!(!file_defines(
            "NESTRS_SEAORM__URL=\nNESTRS_SEAORM__URL=postgres://x\n",
            "NESTRS_SEAORM__URL"
        ));
        assert!(!file_defines(
            "NESTRS_SEAORM__URL=\"\"",
            "NESTRS_SEAORM__URL"
        ));
        assert!(!file_defines("NESTRS_SEAORM__URL=''", "NESTRS_SEAORM__URL"));
    }

    /// A project that renamed its variables must be answered in its own names.
    /// Reporting `<PREFIX>_SEAORM__URL: not set` there is worse than silence:
    /// it sends the reader to add a key the app will never read.
    ///
    /// Hermetic for the same reason as the cascade test above: the default name
    /// is asserted *absent*, which a developer's own `<PREFIX>_SEAORM__URL` would
    /// otherwise contradict from outside the test.
    #[test]
    fn a_custom_prefix_project_is_answered_in_its_own_variable_names() {
        let dir = std::env::temp_dir().join(format!("nestrs-doctor-acme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".env"), "ACME_SEAORM__URL=postgres://x\n").expect("write");
        let cascade = cascade_text(&dir, "ACME", real(&[]));
        assert!(present(real(&[]), &cascade, "ACME_SEAORM__URL"));
        assert!(
            !present(real(&[]), &cascade, "NESTRS_SEAORM__URL"),
            "the default name must not answer for a renamed project",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// config-3r2: a relative `<NAME>_FILE` is opened from the directory the
    /// app starts in, which is the one doctor examines — not doctor's own.
    #[test]
    fn a_relative_file_is_opened_from_the_directory_examined() {
        let dir = scratch("relative");
        std::fs::create_dir_all(dir.join("secrets")).expect("secrets dir");
        std::fs::write(dir.join("secrets/db"), "postgres://x\n").expect("write");
        let cascade = "NESTRS_SEAORM__URL_FILE=secrets/db\n";
        assert_eq!(
            resolve_variable(real(&[]), cascade, "NESTRS_SEAORM__URL", &dir),
            Resolution::Set,
        );
        assert!(matches!(
            resolve_variable(
                real(&[]),
                cascade,
                "NESTRS_SEAORM__URL",
                &dir.join("elsewhere")
            ),
            Resolution::Refused(_)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// config-5r2: the two variables that choose the cascade, written into it,
    /// abort every app at its first config read — refused unless the process
    /// says the same, and named without their value.
    #[test]
    fn a_cascade_naming_what_chooses_it_is_refused() {
        let env = "NESTRS_ENV=production\n";
        let refused = cascade_refusals(real(&[]), env, "NESTRS");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(
            refused[0].starts_with("NESTRS_ENV in the .env cascade")
                && !refused[0].contains("production"),
            "{refused:?}"
        );
        assert!(
            cascade_refusals(real(&[("NESTRS_ENV", "production")]), env, "NESTRS").is_empty(),
            "a file restating what the process carries is redundant, not wrong",
        );
        assert_eq!(
            cascade_refusals(
                real(&[("NESTRS_ENV", "production")]),
                "NESTRS_ENV=\n",
                "NESTRS"
            )
            .len(),
            1,
            "an empty assignment is a value the process does not carry",
        );
        let prefix = cascade_refusals(real(&[]), "NESTRS_ENV_PREFIX=ACME\n", "NESTRS");
        assert!(
            prefix.len() == 1 && prefix[0].starts_with("NESTRS_ENV_PREFIX in the .env cascade"),
            "{prefix:?}"
        );
        assert!(cascade_refusals(real(&[]), "NESTRS_ENV_PREFIX=NESTRS\n", "NESTRS").is_empty());
        assert!(
            cascade_refusals(real(&[]), "ACME_ENV=test\n", "NESTRS").is_empty(),
            "another prefix's selector is no variable of this one",
        );
        assert_eq!(
            cascade_refusals(real(&[]), "ACME_ENV=test\n", "ACME").len(),
            1
        );
    }
}
