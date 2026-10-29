//! A `.env` file's lines, as `nest_rs_config`'s loader reads them: the CLI
//! links no framework crate, so each command that reads one goes through here.

/// Each `KEY=value` line of `contents`, in file order: key and value trimmed,
/// the value still quoted, an `export ` prefix dropped, a comment skipped.
pub(crate) fn assignments(contents: &str) -> impl Iterator<Item = (&str, &str)> {
    contents.lines().filter_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        Some((key.trim(), value.trim()))
    })
}
