//! Transactional file scaffolding.
//!
//! A [`Scaffold`] collects every file a command wants to create and every
//! idempotent edit it wants to apply, then commits them **all-or-nothing**:
//! validation runs first (no `create` may clobber an existing file, every
//! `edit` target must exist), so a failure can never leave a half-written
//! tree. `--dry-run` runs the exact same pipeline but writes nothing and
//! returns the [`Report`] of what *would* change.

use std::fs;
use std::path::{Path, PathBuf};

use super::wiring::Transform;
use crate::error::{CliError, CliResult};

enum Action {
    Create {
        path: PathBuf,
        contents: String,
    },
    /// Like `Create`, but a no-op when the file already exists (shared root
    /// files such as `Justfile`/`.gitignore` when adding an app to a workspace).
    CreateIfMissing {
        path: PathBuf,
        contents: String,
    },
    Edit {
        path: PathBuf,
        transform: Transform,
    },
}

#[derive(Debug, Default)]
pub struct Report {
    pub created: Vec<PathBuf>,
    pub modified: Vec<(PathBuf, Vec<String>)>,
    pub unchanged: Vec<PathBuf>,
    pub dry_run: bool,
}

#[derive(Default)]
pub struct Scaffold {
    actions: Vec<Action>,
}

impl Scaffold {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a brand-new file. Fails at `apply` time if it already exists.
    pub fn create(&mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> &mut Self {
        self.actions.push(Action::Create {
            path: path.into(),
            contents: contents.into(),
        });
        self
    }

    /// Queue a file that is only written when absent (shared root files).
    pub fn create_if_missing(
        &mut self,
        path: impl Into<PathBuf>,
        contents: impl Into<String>,
    ) -> &mut Self {
        self.actions.push(Action::CreateIfMissing {
            path: path.into(),
            contents: contents.into(),
        });
        self
    }

    /// Queue an idempotent edit on an existing file.
    pub fn edit(&mut self, path: impl Into<PathBuf>, transform: Transform) -> &mut Self {
        self.actions.push(Action::Edit {
            path: path.into(),
            transform,
        });
        self
    }

    /// Validate everything, then commit (unless `dry_run`).
    pub fn apply(self, dry_run: bool) -> CliResult<Report> {
        // Phase 1 — validate creates up front so we fail before any write.
        for action in &self.actions {
            if let Action::Create { path, .. } = action
                && path.exists()
            {
                return Err(CliError::AlreadyExists(path.clone()));
            }
        }

        // Phase 2 — resolve edits against current file contents.
        let mut planned: Vec<PlannedWrite> = Vec::new();
        let mut report = Report {
            dry_run,
            ..Report::default()
        };

        for action in self.actions {
            match action {
                Action::Create { path, contents } => {
                    report.created.push(path.clone());
                    planned.push(PlannedWrite { path, contents });
                }
                Action::CreateIfMissing { path, contents } => {
                    if path.exists() {
                        report.unchanged.push(path);
                    } else {
                        report.created.push(path.clone());
                        planned.push(PlannedWrite { path, contents });
                    }
                }
                Action::Edit { path, transform } => {
                    let current = fs::read_to_string(&path).map_err(CliError::Io)?;
                    match transform(&current) {
                        Some(updated) => {
                            let added = added_lines(&current, &updated);
                            report.modified.push((path.clone(), added));
                            planned.push(PlannedWrite {
                                path,
                                contents: updated,
                            });
                        }
                        None => report.unchanged.push(path),
                    }
                }
            }
        }

        // Phase 3 — commit.
        if !dry_run {
            for write in planned {
                if let Some(parent) = write.path.parent() {
                    fs::create_dir_all(parent).map_err(CliError::Io)?;
                }
                fs::write(&write.path, &write.contents).map_err(CliError::Io)?;
            }
        }

        Ok(report)
    }
}

struct PlannedWrite {
    path: PathBuf,
    contents: String,
}

/// Lines present in `new` but not in `old` (used for the wiring diff).
fn added_lines(old: &str, new: &str) -> Vec<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    new.lines()
        .filter(|l| !l.trim().is_empty() && !old_lines.contains(l))
        .map(str::to_string)
        .collect()
}

impl Report {
    /// Print the human-facing summary (`+ created`, `~ modified` + diff).
    pub fn print(&self, base: &Path) {
        if self.dry_run {
            println!("Dry run — no files written.\n");
        }
        for path in &self.created {
            println!("  + {}", rel(path, base));
        }
        for (path, added) in &self.modified {
            println!("  ~ {}", rel(path, base));
            for line in added {
                println!("      {}", line.trim_end());
            }
        }
    }

    /// Every `.rs` file the report touched — for best-effort formatting.
    pub fn rust_files(&self) -> Vec<PathBuf> {
        self.created
            .iter()
            .chain(self.modified.iter().map(|(p, _)| p))
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .cloned()
            .collect()
    }
}

fn rel(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Run `rustfmt` over the given files, best-effort: a missing or failing
/// rustfmt never fails the command (the scaffold already wrote valid code).
pub fn rustfmt(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let outcome = std::process::Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .args(paths)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match outcome {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("note: generated code left unformatted: rustfmt exited with {status}")
        }
        Err(e) => eprintln!("note: generated code left unformatted: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaffold::ensure_decl;

    #[test]
    fn create_is_transactional_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("a.txt");
        fs::write(&existing, "keep").unwrap();

        let mut s = Scaffold::new();
        s.create(dir.path().join("new.txt"), "x");
        s.create(&existing, "overwrite"); // conflict → whole apply fails

        let err = s.apply(false).unwrap_err();
        assert!(matches!(err, CliError::AlreadyExists(_)));
        // nothing partially written
        assert!(!dir.path().join("new.txt").exists());
        assert_eq!(fs::read_to_string(&existing).unwrap(), "keep");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Scaffold::new();
        s.create(dir.path().join("new.txt"), "x");
        let report = s.apply(true).unwrap();
        assert_eq!(report.created.len(), 1);
        assert!(!dir.path().join("new.txt").exists());
    }

    #[test]
    fn edit_records_unchanged_when_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("lib.rs");
        fs::write(&f, "pub mod posts;\n").unwrap();

        let mut s = Scaffold::new();
        s.edit(&f, ensure_decl("pub mod posts;"));
        let report = s.apply(false).unwrap();
        assert_eq!(report.unchanged.len(), 1);
        assert!(report.modified.is_empty());
    }
}
