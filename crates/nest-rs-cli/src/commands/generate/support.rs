//! Shared steps for the `g` generators: commit a generator's scaffold, wire
//! what it generated into the app that serves it, and read the source it
//! edits by whole identifiers.

use std::borrow::Borrow;
use std::path::{Path, PathBuf};

use crate::context::TargetApp;
use crate::error::{CliError, CliResult, FileAction};
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

/// One module the generated code needs the serving app to import: the
/// generated edge, or a backend binding the app's root chooses.
#[derive(Debug, Clone)]
pub(super) struct Import {
    /// The path the `use` line names (`features::posts::PostsHttpModule`).
    pub use_path: String,
    /// The type the `imports = [ … ]` entry names.
    pub ident: String,
}

impl Import {
    pub(super) fn new(use_path: impl Into<String>) -> Self {
        let use_path = use_path.into();
        let ident = use_path.rsplit("::").next().unwrap_or(&use_path).to_owned();
        Self { use_path, ident }
    }
}

/// The unit-of-work binding a `#[crud]` handler runs in, written beside a
/// DB-backed edge into an app that has none; it imports the pool it binds over.
pub(super) fn data_layer() -> Import {
    Import::new("nest_rs::seaorm::SeaOrmDatabaseModule")
}

/// Where the imports went.
#[derive(Debug)]
pub(super) enum Wiring {
    /// Into `app`'s `module.rs`; `written` are the imports it lacked, in the
    /// order given — the rest it already named.
    Wired {
        app: String,
        root: PathBuf,
        written: Vec<String>,
    },
    /// Nowhere: the workspace has no app.
    NoApp,
    /// Nowhere: several apps, and nothing named one.
    Unchosen(Vec<String>),
}

/// Queue `edge` and every import `beside` it the target app's `module.rs` does
/// not already name, in **one** `edit`: a second re-reads the file from disk
/// and clobbers the first. An app module the command cannot read fails it,
/// naming the file.
pub(super) fn wire_into_app(
    target: &TargetApp,
    s: &mut Scaffold,
    edge: &Import,
    beside: &[Import],
) -> CliResult<Wiring> {
    let (name, root) = match target {
        TargetApp::App { name, root } => (name, root),
        TargetApp::NoApp => return Ok(Wiring::NoApp),
        TargetApp::Unchosen(apps) => return Ok(Wiring::Unchosen(apps.clone())),
    };
    let module_rs = root.join("src/module.rs");
    let source = std::fs::read_to_string(&module_rs)
        .map_err(CliError::file(FileAction::Read, &module_rs))?;
    let missing: Vec<&Import> = std::iter::once(edge)
        .chain(beside)
        .filter(|import| !names(&source, &import.ident))
        .collect();
    if !missing.is_empty() {
        let pairs: Vec<(&str, &str)> = missing
            .iter()
            .map(|import| (import.use_path.as_str(), import.ident.as_str()))
            .collect();
        s.edit(&module_rs, ensure_module_imports(&pairs));
    }
    Ok(Wiring::Wired {
        app: name.clone(),
        root: root.clone(),
        written: missing.iter().map(|import| import.ident.clone()).collect(),
    })
}

/// `dir`'s entries, sorted, so a run reads them in one order everywhere.
pub(super) fn list_dir(dir: &Path) -> CliResult<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(CliError::file(FileAction::List, dir))? {
        entries.push(entry.map_err(CliError::file(FileAction::List, dir))?.path());
    }
    entries.sort();
    Ok(entries)
}

/// Whether `source` names `ident` outside its `use` lines: an import listed
/// bare, pinned (`X::for_root(..)`) or of a named instance (`X::<I>`) alike.
fn names(source: &str, ident: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with("use ")
            && !line.starts_with("pub use ")
            && words(line, ident).next().is_some()
    })
}

/// Each byte offset where `word` stands as a whole identifier in `source`.
pub(super) fn words<'a>(source: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    source.match_indices(word).filter_map(move |(at, _)| {
        let before = source[..at].chars().next_back();
        let after = source[at + word.len()..].chars().next();
        (!before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)).then_some(at)
    })
}

pub(super) fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether a `mod.rs` already declares `name` as a child module, in any
/// visibility.
pub(super) fn declares_mod(source: &str, name: &str) -> bool {
    let decl = format!("mod {name};");
    source
        .lines()
        .map(str::trim)
        .any(|line| line == decl || line.ends_with(&format!(" {decl}")))
}

/// The next step saying where `edge` went, for every generator's closing
/// lines: the imports written beside it, or the ones to write by hand when
/// no app was chosen.
pub(super) fn wiring_lines(wiring: &Wiring, edge: &Import, beside: &[Import]) -> Vec<String> {
    match wiring {
        Wiring::Wired { app, written, .. } => {
            let others: Vec<&str> = written
                .iter()
                .map(String::as_str)
                .filter(|ident| *ident != edge.ident)
                .collect();
            let and = if others.is_empty() {
                String::new()
            } else {
                format!(", beside {}", sentence(&others))
            };
            vec![format!("{} is wired into `{app}`{and}.", edge.ident)]
        }
        Wiring::NoApp => vec![format!(
            "Import {} in the app that serves it: this workspace has no app yet \
             (`nestrs new <app>` adds one).",
            to_write(edge, beside),
        )],
        Wiring::Unchosen(apps) => vec![
            format!(
                "Not wired: this workspace has several apps ({}) and none was named.",
                apps.join(", "),
            ),
            format!(
                "Import {} in the one that serves it, or pass `--app <name>` to the next \
                 generator.",
                to_write(edge, beside),
            ),
        ],
    }
}

/// `edge` and every module beside it, as the import lines to write.
fn to_write(edge: &Import, beside: &[Import]) -> String {
    let paths: Vec<String> = std::iter::once(edge)
        .chain(beside)
        .map(|import| format!("`{}`", import.use_path))
        .collect();
    sentence(&paths)
}

/// `a`, `a and b`, `a, b and c`.
fn sentence<S: Borrow<str>>(items: &[S]) -> String {
    match items {
        [] => String::new(),
        [one] => one.borrow().to_owned(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last.borrow()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_listed_bare_pinned_or_named_is_named() {
        for source in [
            "#[module(imports = [SeaOrmModule])]",
            "    imports = [SeaOrmModule::for_root(cfg), Other],",
            "        SeaOrmModule::<Analytics>,",
        ] {
            assert!(names(source, "SeaOrmModule"), "{source}");
        }
    }

    #[test]
    fn a_use_line_or_a_longer_ident_names_nothing() {
        for source in [
            "use nest_rs::seaorm::SeaOrmModule;\n#[module(imports = [])]",
            "#[module(imports = [SeaOrmModuleX, MySeaOrmModule])]",
        ] {
            assert!(!names(source, "SeaOrmModule"), "{source}");
        }
    }

    #[test]
    fn a_sentence_joins_its_last_with_and() {
        assert_eq!(sentence(&["a"]), "a");
        assert_eq!(sentence(&["a", "b"]), "a and b");
        assert_eq!(sentence(&["a", "b", "c"]), "a, b and c");
    }
}
