//! Shared steps for the `g` generators: commit a generator's scaffold, and
//! wire a generated module into the current app.

use std::path::{Path, PathBuf};

use crate::context::Context;
use crate::error::{CliResult, read_if_exists};
use crate::scaffold::{Scaffold, ensure_module_imports, rustfmt};

/// Commit a generator's scaffold, format the touched files, and print the
/// one-line summary plus the change report.
///
/// `what` is the noun phrase alone (`"feature `orders`"`); the tense is this
/// function's, since only it knows whether anything was written.
pub(super) fn finish(s: Scaffold, dry_run: bool, base: &Path, what: &str) -> CliResult<()> {
    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }
    println!("{} {what}", report.verb("Wrote", "Would write"));
    report.print(base);
    Ok(())
}

/// Queue every `(use_path, ident)` import into the app the cursor sits in,
/// returning the edited path. When `require_token` is set, wire only if the
/// app's `module.rs` already contains it — a DB-backed module needs
/// `SeaOrmDatabaseModule`, since mounting it into an app without one compiles
/// yet is refused at boot, naming the binding to import.
///
/// Takes the whole set so `module.rs` gets one `edit`. A `module.rs` the
/// command cannot read fails it, naming the file.
pub(super) fn wire_into_app(
    ctx: &Context,
    s: &mut Scaffold,
    imports: &[(&str, &str)],
    require_token: Option<&str>,
) -> CliResult<Option<PathBuf>> {
    let Some(module_rs) = ctx.current_app_module() else {
        return Ok(None);
    };
    let Some(source) = read_if_exists(&module_rs)? else {
        return Ok(None);
    };
    if require_token.is_some_and(|token| !source.contains(token)) {
        return Ok(None);
    }
    s.edit(&module_rs, ensure_module_imports(imports));
    Ok(Some(module_rs))
}
