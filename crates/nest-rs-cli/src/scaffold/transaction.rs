//! Transactional file scaffolding.
//!
//! A [`Scaffold`] collects every file a command wants to create, move, edit or
//! remove, then commits them **all-or-nothing**: validation runs first (no
//! `create` or move may clobber an existing file, every `edit` target, move
//! source and removed file must exist), and a write the system refuses
//! mid-commit rolls back every one before it, so a failure never leaves a
//! half-written tree.
//! `--dry-run` runs the exact same pipeline but writes nothing and returns
//! the [`Report`] of what *would* change.

use std::fs;
use std::path::{Path, PathBuf};

use super::wiring::Transform;
use crate::error::{CliError, CliResult, FileAction};

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
        /// The edit writes a secret: the report shows each added assignment's
        /// name, never its value.
        secret: bool,
    },
    /// `from` becomes `to`, its contents passed through `transform` (`None`:
    /// unchanged); `to` may not exist.
    Move {
        from: PathBuf,
        to: PathBuf,
        transform: Transform,
    },
    /// The file goes; the rollback puts its bytes back.
    Remove {
        path: PathBuf,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub created: Vec<PathBuf>,
    pub moved: Vec<(PathBuf, PathBuf)>,
    pub removed: Vec<PathBuf>,
    pub modified: Vec<(PathBuf, Vec<String>)>,
    pub unchanged: Vec<PathBuf>,
    pub dry_run: bool,
}

#[derive(Default)]
pub(crate) struct Scaffold {
    actions: Vec<Action>,
}

impl Scaffold {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Queue a brand-new file. Fails at `apply` time if it already exists.
    pub(crate) fn create(
        &mut self,
        path: impl Into<PathBuf>,
        contents: impl Into<String>,
    ) -> &mut Self {
        self.actions.push(Action::Create {
            path: path.into(),
            contents: contents.into(),
        });
        self
    }

    /// Queue a file that is only written when absent (shared root files).
    pub(crate) fn create_if_missing(
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
    pub(crate) fn edit(&mut self, path: impl Into<PathBuf>, transform: Transform) -> &mut Self {
        self.actions.push(Action::Edit {
            path: path.into(),
            transform,
            secret: false,
        });
        self
    }

    /// Queue an idempotent edit that adds a secret: the report prints each added
    /// `KEY=value` as `KEY=<redacted>`, so a terminal or a CI log never holds it.
    pub(crate) fn edit_secret(
        &mut self,
        path: impl Into<PathBuf>,
        transform: Transform,
    ) -> &mut Self {
        self.actions.push(Action::Edit {
            path: path.into(),
            transform,
            secret: true,
        });
        self
    }

    /// Queue a move of `from` to `to`, rewritten by `transform` on the way.
    /// Fails at `apply` time if `to` exists.
    pub(crate) fn move_to(
        &mut self,
        from: impl Into<PathBuf>,
        to: impl Into<PathBuf>,
        transform: Transform,
    ) -> &mut Self {
        self.actions.push(Action::Move {
            from: from.into(),
            to: to.into(),
            transform,
        });
        self
    }

    /// Queue the removal of a file that exists.
    pub(crate) fn remove(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.actions.push(Action::Remove { path: path.into() });
        self
    }

    /// Validate everything, then commit (unless `dry_run`).
    pub(crate) fn apply(self, dry_run: bool) -> CliResult<Report> {
        // Phase 1 — validate creates and moves up front so we fail before any
        // write.
        for action in &self.actions {
            let target = match action {
                Action::Create { path, .. } => path,
                Action::Move { to, .. } => to,
                Action::Remove { path } if !path.is_file() => {
                    return Err(CliError::file(FileAction::Remove, path)(
                        std::io::ErrorKind::NotFound.into(),
                    ));
                }
                Action::CreateIfMissing { .. } | Action::Edit { .. } | Action::Remove { .. } => {
                    continue;
                }
            };
            if target.exists() {
                return Err(CliError::AlreadyExists(target.clone()));
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
                    planned.push(PlannedWrite::Write { path, contents });
                }
                Action::CreateIfMissing { path, contents } => {
                    if path.exists() {
                        report.unchanged.push(path);
                    } else {
                        report.created.push(path.clone());
                        planned.push(PlannedWrite::Write { path, contents });
                    }
                }
                Action::Move {
                    from,
                    to,
                    transform,
                } => {
                    let current = fs::read_to_string(&from)
                        .map_err(CliError::file(FileAction::Read, &from))?;
                    let contents = transform(&current).unwrap_or(current);
                    report.moved.push((from.clone(), to.clone()));
                    planned.push(PlannedWrite::Write { path: to, contents });
                    planned.push(PlannedWrite::Remove { path: from });
                }
                Action::Remove { path } => {
                    report.removed.push(path.clone());
                    planned.push(PlannedWrite::Remove { path });
                }
                Action::Edit {
                    path,
                    transform,
                    secret,
                } => {
                    let current = fs::read_to_string(&path)
                        .map_err(CliError::file(FileAction::Read, &path))?;
                    match transform(&current) {
                        Some(updated) => {
                            let mut added = added_lines(&current, &updated);
                            if secret {
                                added = added.into_iter().map(redact_assignment).collect();
                            }
                            report.modified.push((path.clone(), added));
                            planned.push(PlannedWrite::Write {
                                path,
                                contents: updated,
                            });
                        }
                        None => report.unchanged.push(path),
                    }
                }
            }
        }

        // Phase 3 — commit, undoing every step taken when one is refused.
        if !dry_run {
            let mut undo = Vec::new();
            for write in planned {
                if let Err(refused) = write.commit(&mut undo) {
                    return Err(roll_back(undo, refused));
                }
            }
        }

        Ok(report)
    }
}

enum PlannedWrite {
    Write { path: PathBuf, contents: String },
    Remove { path: PathBuf },
}

/// What restores the tree to before one committed step.
enum Undo {
    /// Delete a file the commit created, or was refused creating.
    Delete(PathBuf),
    /// Put back the bytes a file held.
    Restore(PathBuf, Vec<u8>),
    /// Remove a directory the commit created.
    RemoveDir(PathBuf),
}

impl PlannedWrite {
    /// Commit this step, pushing onto `undo` what reverses each part done.
    fn commit(self, undo: &mut Vec<Undo>) -> CliResult<()> {
        match self {
            Self::Write { path, contents } => {
                if let Some(parent) = path.parent() {
                    let created: Vec<PathBuf> = parent
                        .ancestors()
                        .take_while(|dir| !dir.as_os_str().is_empty() && !dir.exists())
                        .map(Path::to_path_buf)
                        .collect();
                    fs::create_dir_all(parent)
                        .map_err(CliError::file(FileAction::CreateDir, parent))?;
                    undo.extend(created.into_iter().rev().map(Undo::RemoveDir));
                }
                match fs::read(&path) {
                    Ok(before) => undo.push(Undo::Restore(path.clone(), before)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        undo.push(Undo::Delete(path.clone()));
                    }
                    Err(e) => return Err(CliError::file(FileAction::Read, &path)(e)),
                }
                fs::write(&path, contents).map_err(CliError::file(FileAction::Write, &path))
            }
            Self::Remove { path } => {
                let before = fs::read(&path).map_err(CliError::file(FileAction::Read, &path))?;
                fs::remove_file(&path).map_err(CliError::file(FileAction::Remove, &path))?;
                undo.push(Undo::Restore(path, before));
                Ok(())
            }
        }
    }
}

/// Reverse every committed step, last first, and hand back `refused`; a step
/// that will not reverse is named beside it rather than dropped.
fn roll_back(undo: Vec<Undo>, refused: CliError) -> CliError {
    let mut left = Vec::new();
    for step in undo.iter().rev() {
        let (path, outcome) = match step {
            // A refused write may have created nothing: the file is already absent.
            Undo::Delete(path) => (
                path,
                fs::remove_file(path).or_else(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => Ok(()),
                    _ => Err(e),
                }),
            ),
            Undo::Restore(path, bytes) => (path, fs::write(path, bytes)),
            Undo::RemoveDir(path) => (path, fs::remove_dir(path)),
        };
        if let Err(e) = outcome {
            left.push(format!("`{}` ({e})", path.display()));
        }
    }
    if left.is_empty() {
        refused
    } else {
        CliError::RollbackIncomplete {
            left: left.join(", "),
            source: Box::new(refused),
        }
    }
}

/// A `.env` assignment with its value withheld; a comment or any other line as is.
fn redact_assignment(line: String) -> String {
    match line.split_once('=') {
        Some((name, _)) if !line.trim_start().starts_with('#') => format!("{name}=<redacted>"),
        _ => line,
    }
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
    /// The tense to announce this report in — `verb("Created", "Would create")`.
    ///
    /// Read off the report rather than the caller's `dry_run` flag: only the commit
    /// knows which it was.
    pub(crate) fn verb(&self, past: &'static str, conditional: &'static str) -> &'static str {
        if self.dry_run { conditional } else { past }
    }

    /// Print the human-facing summary (`+ created`, `~ modified` + diff).
    pub(crate) fn print(&self, base: &Path) {
        if self.dry_run {
            println!("Dry run — no files written.\n");
        }
        for path in &self.created {
            println!("  + {}", rel(path, base));
        }
        for (from, to) in &self.moved {
            println!("  > {} → {}", rel(from, base), rel(to, base));
        }
        for path in &self.removed {
            println!("  - {}", rel(path, base));
        }
        for (path, added) in &self.modified {
            println!("  ~ {}", rel(path, base));
            for line in added {
                println!("      {}", line.trim_end());
            }
        }
    }

    /// Every `.rs` file the report touched — for best-effort formatting.
    pub(crate) fn rust_files(&self) -> Vec<PathBuf> {
        self.created
            .iter()
            .chain(self.moved.iter().map(|(_, to)| to))
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
pub(crate) fn rustfmt(paths: &[PathBuf]) {
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

/// `source` as the `rustfmt` [`rustfmt`] runs would leave it, `None` when it
/// cannot run: what a generator wrote, to compare a file on disk with.
pub(crate) fn rustfmt_source(source: &str) -> Option<String> {
    use std::io::Write;

    let mut child = std::process::Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(source.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
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
    fn an_edit_holding_a_secret_reports_its_variable_and_never_its_value() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(".env.local");
        fs::write(&f, "APP_LOG=debug\n").unwrap();

        let mut s = Scaffold::new();
        s.edit_secret(
            &f,
            Box::new(|content: &str| Some(format!("{content}# a key\nAPP_KEY=s3cr3t\n"))),
        );
        let report = s.apply(true).unwrap();

        let (_, added) = &report.modified[0];
        assert_eq!(added, &["# a key", "APP_KEY=<redacted>"]);
    }

    #[test]
    fn a_move_rewrites_the_file_on_its_way_and_a_dry_run_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("entity.rs");
        let to = dir.path().join("entities/post.rs");
        fs::write(&from, "use super::service::PostsService;\n").unwrap();
        let deepen =
            || -> Transform { Box::new(|c: &str| Some(c.replace("super::", "super::super::"))) };

        let mut s = Scaffold::new();
        s.move_to(&from, &to, deepen());
        let report = s.apply(true).unwrap();
        assert_eq!(report.moved, vec![(from.clone(), to.clone())]);
        assert!(from.is_file() && !to.exists(), "a dry run moves nothing");

        let mut s = Scaffold::new();
        s.move_to(&from, &to, deepen());
        s.apply(false).unwrap();
        assert!(!from.exists());
        assert_eq!(
            fs::read_to_string(&to).unwrap(),
            "use super::super::service::PostsService;\n"
        );
    }

    #[test]
    fn a_refused_write_rolls_back_every_step_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("entity.rs");
        let edited = dir.path().join("mod.rs");
        fs::write(&from, "pub struct Model;\n").unwrap();
        fs::write(&edited, "mod entity;\n").unwrap();
        // A file where the last write needs a directory: the commit is refused there.
        fs::write(dir.path().join("blocked"), "").unwrap();

        let mut s = Scaffold::new();
        s.move_to(
            &from,
            dir.path().join("entities/post.rs"),
            Box::new(|_| None),
        );
        s.edit(&edited, ensure_decl("mod entities;"));
        s.create(dir.path().join("fresh/new.rs"), "x");
        s.create(dir.path().join("blocked/late.rs"), "x");
        let refused = s.apply(false).unwrap_err();

        assert!(
            crate::error::chain(&refused).contains("blocked"),
            "{}",
            crate::error::chain(&refused)
        );
        assert_eq!(fs::read_to_string(&from).unwrap(), "pub struct Model;\n");
        assert_eq!(fs::read_to_string(&edited).unwrap(), "mod entity;\n");
        for gone in ["entities", "fresh"] {
            assert!(!dir.path().join(gone).exists(), "{gone} is rolled back");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_write_of_a_new_file_rolls_back_whole_and_reports_the_refusal_alone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let edited = dir.path().join("mod.rs");
        fs::write(&edited, "mod service;\n").unwrap();
        let locked = dir.path().join("notes");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

        let mut s = Scaffold::new();
        s.edit(&edited, ensure_decl("mod entity;"));
        s.create(locked.join("entity.rs"), "pub struct Model;\n");
        let refused = s.apply(false).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            matches!(
                &refused,
                CliError::File {
                    action: FileAction::Write,
                    ..
                }
            ),
            "the write is the refusal, and the rollback left nothing: {}",
            crate::error::chain(&refused)
        );
        assert_eq!(fs::read_to_string(&edited).unwrap(), "mod service;\n");
        assert!(!locked.join("entity.rs").exists());
    }

    #[test]
    fn a_move_onto_an_existing_file_is_refused_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("entity.rs");
        let to = dir.path().join("post.rs");
        fs::write(&from, "a").unwrap();
        fs::write(&to, "b").unwrap();

        let mut s = Scaffold::new();
        s.create(dir.path().join("new.rs"), "x");
        s.move_to(&from, &to, Box::new(|_| None));
        assert!(matches!(
            s.apply(false).unwrap_err(),
            CliError::AlreadyExists(_)
        ));
        assert!(!dir.path().join("new.rs").exists());
        assert_eq!(fs::read_to_string(&to).unwrap(), "b");
    }

    #[test]
    fn a_removed_file_comes_back_when_a_later_step_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let subject = dir.path().join("subject.rs");
        fs::write(&subject, "pub struct Post;\n").unwrap();
        fs::write(dir.path().join("blocked"), "").unwrap();

        let mut s = Scaffold::new();
        s.remove(&subject);
        let report = s.apply(true).unwrap();
        assert_eq!(report.removed, vec![subject.clone()]);
        assert!(subject.is_file(), "a dry run removes nothing");

        let mut s = Scaffold::new();
        s.remove(&subject);
        s.create(dir.path().join("blocked/late.rs"), "x");
        s.apply(false).unwrap_err();
        assert_eq!(fs::read_to_string(&subject).unwrap(), "pub struct Post;\n");

        let mut s = Scaffold::new();
        s.remove(dir.path().join("absent.rs"));
        assert!(matches!(
            s.apply(false).unwrap_err(),
            CliError::File {
                action: FileAction::Remove,
                ..
            }
        ));
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
