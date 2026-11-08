use std::path::PathBuf;

use crate::error::{CliError, CliResult, FileAction};
use crate::lint::scan;

use super::resolve_start;

pub(crate) struct LintOptions {
    pub path: Option<PathBuf>,
}

/// Read every `src/` file under the given root and report each one whose stem
/// reaches nothing it declares.
///
/// A caller that wants the findings as data calls [`crate::lint::scan`].
pub(crate) fn run(opts: LintOptions) -> CliResult<()> {
    let scan = scan(&resolve_start(opts.path));

    println!("nestrs lint");
    println!();
    if let Some((path, source)) = scan.unreadable.into_iter().next() {
        return Err(CliError::File {
            action: FileAction::Read,
            path,
            source,
        });
    }
    if scan.findings.is_empty() && scan.unparsable.is_empty() {
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
    for (path, refused) in &scan.unparsable {
        // The parser counts columns from zero; a location reads from one, as
        // rustc writes it.
        let at = refused.span().start();
        println!(
            "  {}:{}:{}\n    does not parse: {refused}\n    \
             a file the parser refuses is a file this rule could not judge",
            path.display(),
            at.line,
            at.column + 1,
        );
        println!();
    }
    let mut failed = Vec::new();
    let n = scan.findings.len();
    if n > 0 {
        failed.push(format!(
            "{n} of {} files {} named for a slot rather than a subject",
            scan.checked,
            if n == 1 { "is" } else { "are" },
        ));
    }
    let unparsable = scan.unparsable.len();
    if unparsable > 0 {
        failed.push(format!(
            "{unparsable} {} not parse",
            if unparsable == 1 {
                "file does"
            } else {
                "files do"
            },
        ));
    }
    Err(CliError::Anyhow(anyhow::anyhow!("{}", failed.join("; "))))
}
