use std::fmt;
use std::path::PathBuf;

/// One file whose stem and whose declared types do not reach each other.
///
/// Carries the halves rather than a rendered sentence: the command prints it,
/// the conformance suite asserts on it, and a caller that only wants the paths
/// should not have to parse English to get them.
#[derive(Debug)]
pub struct Finding {
    /// The offending file, relative to the root the scan was given.
    pub path: PathBuf,
    /// Every type the file declares, in source order.
    pub declared: Vec<String>,
}

impl Finding {
    /// The word that reaches nothing — the file's own stem, which is where the
    /// rule reads it from, so it is not carried a second time as a field.
    pub fn stem(&self) -> std::borrow::Cow<'_, str> {
        self.path.file_stem().unwrap_or_default().to_string_lossy()
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\n    `{}` reaches none of `{}`\n    \
             name the file from the type, or split it — a stem that reaches nothing \
             is a slot, and a slot fills",
            self.path.display(),
            self.stem(),
            self.declared.join("`, `"),
        )
    }
}
