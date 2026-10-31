use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// What a command was doing to a file when the system refused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    /// Reading its contents.
    Read,
    /// Writing its contents.
    Write,
    /// Creating it as a directory, with its parents.
    CreateDir,
    /// Listing a directory's entries.
    List,
    /// Removing it.
    Remove,
    /// Resolving it to an absolute path.
    Resolve,
}

impl fmt::Display for FileAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::CreateDir => "create the directory",
            Self::List => "list",
            Self::Remove => "remove",
            Self::Resolve => "resolve",
        })
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CliError {
    #[error("path already exists: {}", .0.display())]
    AlreadyExists(PathBuf),

    #[error("app `{name}` already exists at {path}")]
    AppExists { name: String, path: PathBuf },

    /// `--app` named no app of the workspace.
    #[error("no app `{name}` in this workspace — its apps: {apps}")]
    AppNotFound { name: String, apps: String },

    #[error(
        "not inside a nestrs workspace (expected root Cargo.toml with members = [\"crates/*\", \"apps/*\"])"
    )]
    NotNestrsWorkspace,

    #[error("feature `{name}` already exists at {path}")]
    FeatureExists { name: String, path: PathBuf },

    #[error("feature `{name}` not found — create it first with `nestrs g feature {name}`")]
    FeatureNotFound { name: String },

    /// `g entity` adding a second entity to a feature whose lone `entity.rs`
    /// cannot move to `entities/` with certainty: a reference the move would
    /// break, by file and line.
    #[error(
        "feature `{feature}` keeps its entity in `entity.rs`, and a second one moves it to \
         `entities/{stem}.rs` — but `{at}` holds {what}, which the move cannot rewrite with \
         certainty. Point it at `entities::{stem}` yourself (a `super::` path in the moved \
         file gains one level), then re-run"
    )]
    EntityMoveUncertain {
        feature: String,
        stem: String,
        at: String,
        what: &'static str,
    },

    /// `g entity` on a port whose `subject.rs` declares the entity's name.
    ///
    /// The subject's explicit `pub use` would shadow the wire type the entity's
    /// glob re-exports; re-gating the operations is left to the developer
    /// (`commands::generate::entity`).
    #[error(
        "feature `{feature}` declares `{name}` in `subject.rs`, the capability its \
         operations are gated on, and the entity's wire type takes that name: \
         `pub use subject::{name};` would shadow it. Retire the subject first, then \
         re-run:\n  \
         1. delete crates/features/src/{feature}/subject.rs, and `mod subject;` / \
         `pub use subject::{name};` in `{feature}/mod.rs`\n  \
         2. nestrs g entity {target}\n  \
         3. re-gate on the entity every operation gated on `{name}`, with \
         `use crate::{feature}::Entity as {name}Entity;` and \
         `#[authorize(Read, {name}Entity)]` (`g graphql` wrote one in \
         `graphql/resolver.rs`, `g mcp` in `mcp/tool.rs`), and grant \
         `crate::{feature}::Entity` in `authz/ability.rs` where you granted \
         `crate::{feature}::{name}`"
    )]
    EntityNameTakenBySubject {
        feature: String,
        name: String,
        target: String,
    },

    #[error(
        "`{0}` is not a `<feature>` or `<feature>/<entity>` target — one optional `/`, no more"
    )]
    InvalidEntityTarget(String),

    #[error("{0}")]
    InvalidFeatureName(String),

    /// A migration's name, or the table a migration, a resource or an entity
    /// writes, taking the framework's `nestrs_` prefix for a migration the
    /// framework does not ship.
    #[error(
        "`{name}` claims the `nestrs_` prefix, which names the framework's own tables: \
         `nestrs g migration` registers under it only a migration the framework ships — \
         {accepted}"
    )]
    ReservedPrefix { name: String, accepted: String },

    #[error("{transport} adapter for `{name}` already exists at {path}")]
    AdapterExists {
        transport: &'static str,
        name: String,
        path: PathBuf,
    },

    /// A file the command reads or writes, named with what it was doing.
    #[error("cannot {action} `{}`", path.display())]
    File {
        action: FileAction,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A program the command starts, which did not start.
    #[error("cannot run `{program}`")]
    Spawn {
        program: &'static str,
        #[source]
        source: std::io::Error,
    },

    /// A commit refused midway, whose rollback could not reverse every step.
    #[error("the scaffold stopped midway, and its rollback left these as written: {left}")]
    RollbackIncomplete {
        left: String,
        #[source]
        source: Box<CliError>,
    },

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

impl CliError {
    /// The [`CliError::File`] an io error on `path` becomes, for a `map_err`.
    pub(crate) fn file(action: FileAction, path: &Path) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| Self::File {
            action,
            path,
            source,
        }
    }

    /// The [`CliError::Spawn`] an io error starting `program` becomes.
    pub(crate) fn spawn(program: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Spawn { program, source }
    }
}

pub type CliResult<T> = Result<T, CliError>;

/// `error` and every cause below it, joined by `: ` — what `main` prints, so a
/// failure reads down to the system's own answer.
pub fn chain(error: &dyn std::error::Error) -> String {
    let mut line = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        line.push_str(": ");
        line.push_str(&next.to_string());
        cause = next.source();
    }
    line
}

/// `path`'s contents, or `None` when there is no such file; any other
/// refusal names the file.
pub(crate) fn read_if_exists(path: &Path) -> CliResult<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::file(FileAction::Read, path)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_error_reads_down_to_the_system_s_answer() {
        let err = CliError::file(FileAction::Read, Path::new("apps/blog/src/module.rs"))(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        let printed = chain(&err);
        assert!(
            printed.starts_with("cannot read `apps/blog/src/module.rs`: "),
            "{printed}"
        );
        assert!(printed.contains("ermission denied"), "{printed}");
    }

    #[test]
    fn an_anyhow_context_keeps_its_cause() {
        let cause = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let err = CliError::Anyhow(anyhow::Error::new(cause).context("cannot read `.env.test`"));
        let printed = chain(&err);
        assert!(
            printed.starts_with("cannot read `.env.test`: "),
            "{printed}"
        );
        assert!(printed.contains("ermission denied"), "{printed}");
    }

    #[test]
    fn a_missing_file_is_none_and_a_directory_is_named() {
        let dir = tempfile::tempdir().expect("a temp dir");
        assert!(
            read_if_exists(&dir.path().join("absent.rs"))
                .expect("absent")
                .is_none()
        );
        let refused = read_if_exists(dir.path()).expect_err("a directory is no file");
        assert!(
            chain(&refused).contains(&dir.path().display().to_string()),
            "{}",
            chain(&refused)
        );
    }
}
