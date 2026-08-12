//! `nestrs info` — what **this project** is.
//!
//! The split with `nestrs about` is the reason both exist. `about` answers *what
//! NestRS is* — version, tagline, docs, licence, author — and prints the same
//! lines on every machine in every directory. `info` answers *where you are
//! standing*: which layout, which root, which apps and features it holds, the
//! framework line its manifests pin, the env prefix in force, and the toolchain
//! that will build it. Every line here needs the tree read; no line in `about`
//! does.
//!
//! It never fails on "there is no project". A reader who ran it in the wrong
//! directory is told so plainly — that is the answer they asked for, and an
//! error exit would make `info` unusable as the first thing you type.

use std::path::{Path, PathBuf};

use crate::context::{
    Context, DEFAULT_ENV_PREFIX, ENV_PREFIX_VAR, EnvPrefixSource, NestrsWorkspace, StandaloneCrate,
    framework_pin,
};
use crate::error::CliResult;

pub struct InfoOptions {
    pub path: Option<PathBuf>,
}

pub fn run(opts: InfoOptions) -> CliResult<()> {
    let start = opts
        .path
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let here = start.canonicalize().unwrap_or_else(|_| start.clone());
    let ctx = Context::detect(&start)?;

    println!("NestRS project");
    match &ctx.workspace {
        Some(ws) => print_workspace(ws, &ctx, &here)?,
        // A crate inside a workspace is a member, so the standalone probe only
        // runs once workspace discovery has come back empty.
        None => match StandaloneCrate::discover(&start)? {
            Some(krate) => print_standalone(&krate, &here)?,
            None => row("Layout", "none — not inside a nestrs workspace or crate"),
        },
    }

    // Read from *this* environment, which is the source an app started here
    // would read — the same reason `doctor` reports it rather than resolving it
    // silently.
    row("Env prefix", &env_prefix_line());
    row(
        "Toolchain",
        super::doctor::rustc_version()
            .as_deref()
            .unwrap_or("rustc not found"),
    );
    row("CLI", env!("CARGO_PKG_VERSION"));
    Ok(())
}

fn print_workspace(ws: &NestrsWorkspace, ctx: &Context, here: &Path) -> CliResult<()> {
    row("Layout", "workspace");
    row("Name", &project_name(&ws.root));
    row("Root", &relative(&ws.root, here));
    row("Framework", &framework_line(&ws.root)?);
    row("Apps", &list(&dir_names(&ws.apps_root())));
    row("Features", &list(&dir_names(&ws.features_root())));
    // Which app a generator would wire into — the one thing about the cursor
    // that changes what `nestrs g` does.
    if let Some(app) = ctx.current_app.as_ref().and_then(|app| app.file_name()) {
        row("Current app", &app.to_string_lossy());
    }
    Ok(())
}

fn print_standalone(krate: &StandaloneCrate, here: &Path) -> CliResult<()> {
    row("Layout", "standalone crate");
    row("Name", &krate.name);
    row("Root", &relative(&krate.root, here));
    row("Framework", &framework_line(&krate.root)?);
    Ok(())
}

/// One `label:` / value pair, in `about`'s column.
fn row(label: &str, value: &str) {
    println!("{:<15}{value}", format!("{label}:"));
}

fn framework_line(root: &Path) -> CliResult<String> {
    Ok(match framework_pin(root)? {
        Some(req) => format!("nest-rs {req}"),
        None => "nest-rs (no version pinned in Cargo.toml)".to_owned(),
    })
}

fn env_prefix_line() -> String {
    match EnvPrefixSource::detect() {
        EnvPrefixSource::Environment(prefix) => format!("{prefix} (from {ENV_PREFIX_VAR})"),
        EnvPrefixSource::Unset => {
            format!("{DEFAULT_ENV_PREFIX} (default — {ENV_PREFIX_VAR} names none here)")
        }
        EnvPrefixSource::Invalid(reason) => format!("{ENV_PREFIX_VAR} is unusable — {reason}"),
    }
}

/// The project's name: the workspace directory's own, since the project name
/// stops at the workspace and appears nowhere below it.
fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// `root` as seen from `here` — `.` when they are the same, otherwise the climb.
///
/// The absolute path is a machine-local detail: a report worth pasting into an
/// issue should not carry someone's home directory, and the relative form is
/// also the one a reader can act on. The absolute path is the fallback for the
/// case that cannot be relativized, which discovery makes unreachable.
fn relative(root: &Path, here: &Path) -> String {
    match here.strip_prefix(root) {
        Ok(rest) => match rest.components().count() {
            0 => ".".to_owned(),
            ups => vec![".."; ups].join("/"),
        },
        Err(_) => root.display().to_string(),
    }
}

/// Sub-directory names, sorted. Dot-directories are excluded: nothing the
/// layout defines starts with one, so they are always someone's tooling.
fn dir_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

fn list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_owned()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_reported_relative_to_where_the_reader_stands() {
        let root = Path::new("/w/acme");
        assert_eq!(relative(root, Path::new("/w/acme")), ".");
        assert_eq!(relative(root, Path::new("/w/acme/apps")), "..");
        assert_eq!(
            relative(root, Path::new("/w/acme/apps/api/src")),
            "../../.."
        );
    }

    // A report that leaks `/home/<someone>` is one nobody can paste into an
    // issue; only a root the climb cannot reach falls back to the absolute path.
    #[test]
    fn an_unrelated_root_falls_back_to_the_path_it_has() {
        assert_eq!(
            relative(Path::new("/w/acme"), Path::new("/elsewhere")),
            "/w/acme"
        );
    }

    #[test]
    fn an_empty_directory_reads_as_none_rather_than_a_blank() {
        assert_eq!(list(&[]), "(none)");
        assert_eq!(
            list(&["api".to_owned(), "worker".to_owned()]),
            "api, worker"
        );
    }
}
