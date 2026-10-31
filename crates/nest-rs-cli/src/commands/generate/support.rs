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
/// generated edge, or what it runs on that the app's root chooses.
#[derive(Debug, Clone)]
pub(super) struct Import {
    /// The path the `use` line names (`features::posts::PostsHttpModule`).
    pub use_path: String,
    /// The type the `imports = [ … ]` entry names.
    pub ident: String,
    /// The binding this resource is opened for, which reads its default
    /// instance (`RedisQueueModule` for `RedisModule`).
    opened_for: Option<&'static str>,
}

impl Import {
    pub(super) fn new(use_path: impl Into<String>) -> Self {
        let use_path = use_path.into();
        let ident = use_path.rsplit("::").next().unwrap_or(&use_path).to_owned();
        Self {
            use_path,
            ident,
            opened_for: None,
        }
    }

    /// This resource, written only beside `binding`, and only into a root
    /// that does not open its default instance: an instance named
    /// (`X::<I>`) opens another one, which the binding never reads.
    pub(super) fn opened_for(self, binding: &'static str) -> Self {
        Self {
            opened_for: Some(binding),
            ..self
        }
    }

    /// Whether the root `source` already provides what this import brings.
    fn provided_by(&self, source: &str) -> bool {
        match self.opened_for {
            None => names(source, &self.ident),
            Some(binding) => names(source, binding) || opens_default(source, &self.ident),
        }
    }
}

/// The unit-of-work binding a `#[crud]` handler runs in, and the pool it
/// binds over: the data layer, written beside a DB-backed edge into an app
/// that has none.
pub(super) fn data_layer() -> [Import; 2] {
    [
        Import::new("nest_rs::seaorm::SeaOrmModule"),
        Import::new("nest_rs::seaorm::SeaOrmDatabaseModule"),
    ]
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
        .filter(|import| !import.provided_by(&source))
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

/// Whether `source` names `ident` outside its `use` lines: an import listed
/// bare, pinned (`X::for_root(..)`) or of a named instance (`X::<I>`) alike.
fn names(source: &str, ident: &str) -> bool {
    listed(source).any(|line| words(line, ident).next().is_some())
}

/// Whether `source` opens `ident`'s default instance: lists it bare, as
/// `X::<Unnamed>` or pinned by a value (`X::for_root(x)`) — never only a named
/// instance's `X::<I>` or `X::for_root((I, x))`.
fn opens_default(source: &str, ident: &str) -> bool {
    let listed = listed(source).collect::<Vec<_>>().join("\n");
    words(&listed, ident).any(|at| {
        let Some(path) = listed[at + ident.len()..].trim_start().strip_prefix("::") else {
            return true;
        };
        let path = path.trim_start();
        if let Some(generic) = path.strip_prefix('<') {
            let instance = generic.split('>').next().unwrap_or_default().trim();
            return instance.rsplit("::").next() == Some("Unnamed");
        }
        path.strip_prefix("for_root")
            .and_then(|call| call.trim_start().strip_prefix('('))
            .is_none_or(|value| !value.trim_start().starts_with('('))
    })
}

/// The lines of `source` that can list an import: every one but its `use`s.
fn listed(source: &str) -> impl Iterator<Item = &str> {
    source.lines().filter(|line| {
        let line = line.trim_start();
        !line.starts_with("use ") && !line.starts_with("pub use ")
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
    fn the_default_instance_is_opened_bare_unnamed_or_pinned_by_a_value() {
        for source in [
            "#[module(imports = [RedisModule])]",
            "        RedisModule::<Unnamed>,",
            "        RedisModule::<nest_rs::core::Unnamed>,",
            "        RedisModule::for_root(redis()),",
            "        RedisModule::<Cache>,\n        RedisModule,",
        ] {
            assert!(opens_default(source, "RedisModule"), "{source}");
        }
    }

    #[test]
    fn a_named_instance_alone_leaves_the_default_unopened() {
        for source in [
            "        RedisModule::<Cache>,",
            "        RedisModule::for_root((Cache, cache())),",
            "        RedisModule::for_root(\n            (Cache, cache()),\n        ),",
            "use nest_rs::redis::RedisModule;\n#[module(imports = [])]",
        ] {
            assert!(!opens_default(source, "RedisModule"), "{source}");
        }
    }

    #[test]
    fn a_resource_opened_for_a_binding_is_provided_by_the_binding_or_its_default() {
        let redis = || Import::new("nest_rs::redis::RedisModule").opened_for("RedisQueueModule");
        assert!(
            redis().provided_by("        RedisModule::<Jobs>,\n        RedisQueueModule::<Jobs>,")
        );
        assert!(redis().provided_by("        RedisModule::for_root(redis()),"));
        assert!(!redis().provided_by("        RedisModule::<Cache>,"));
    }

    #[test]
    fn a_sentence_joins_its_last_with_and() {
        assert_eq!(sentence(&["a"]), "a");
        assert_eq!(sentence(&["a", "b"]), "a and b");
        assert_eq!(sentence(&["a", "b", "c"]), "a, b and c");
    }
}
