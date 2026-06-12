//! Shared steps for the `g` generators: resolve the working directory, commit
//! a generator's scaffold, and wire a generated module into the current app.

use std::path::{Path, PathBuf};

use crate::context::Context;
use crate::error::CliResult;
use crate::scaffold::{Scaffold, ensure_module_import, rustfmt};

/// Resolve a generator's working directory (explicit `-p` or the cwd).
pub(super) fn resolve_start(path: Option<PathBuf>) -> PathBuf {
    path.unwrap_or_else(|| std::env::current_dir().expect("cwd"))
}

/// Commit a generator's scaffold, format the touched files, and print the
/// one-line summary plus the change report.
pub(super) fn finish(s: Scaffold, dry_run: bool, base: &Path, summary: &str) -> CliResult<()> {
    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }
    println!("{summary}");
    report.print(base);
    Ok(())
}

/// Queue an import of `ident` into the app the cursor sits in, returning the
/// edited path. When `require_token` is set, wire only if the app's `module.rs`
/// already contains it — a DB-backed module needs `DatabaseModule`, since
/// mounting it into an app without one compiles yet panics at boot.
pub(super) fn wire_into_app(
    ctx: &Context,
    s: &mut Scaffold,
    use_path: &str,
    ident: &str,
    require_token: Option<&str>,
) -> Option<PathBuf> {
    let module_rs = ctx.current_app_module()?;
    if !module_rs.is_file() {
        return None;
    }
    if let Some(token) = require_token {
        let source = std::fs::read_to_string(&module_rs).ok()?;
        if !source.contains(token) {
            return None;
        }
    }
    s.edit(&module_rs, ensure_module_import(use_path, ident));
    Some(module_rs)
}
