use std::path::PathBuf;

use crate::error::{CliError, CliResult};
use crate::lint::scan;

use super::resolve_start;

pub struct LintOptions {
    pub path: Option<PathBuf>,
}

/// Read every `src/` file under the given root and report each one whose stem
/// reaches nothing it declares.
///
/// The command's job is the report and the exit code: a lint nobody's CI can
/// fail is a lint nobody runs. A caller that wants the findings as data calls
/// [`crate::lint::scan`], which is what `nest-rs-conformance` does.
pub fn run(opts: LintOptions) -> CliResult<()> {
    let scan = scan(&resolve_start(opts.path));

    println!("nestrs lint");
    println!();
    if scan.findings.is_empty() {
        println!(
            "  every one of the {} files checked is named for what it declares",
            scan.checked,
        );
        return Ok(());
    }

    for finding in &scan.findings {
        println!("  {finding}");
        println!();
    }
    let n = scan.findings.len();
    Err(CliError::Anyhow(anyhow::anyhow!(
        "{n} of {} files {} named for a slot rather than a subject",
        scan.checked,
        if n == 1 { "is" } else { "are" },
    )))
}
