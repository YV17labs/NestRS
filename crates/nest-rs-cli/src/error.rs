use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("path already exists: {}", .0.display())]
    AlreadyExists(PathBuf),

    #[error("not inside a nestrs workspace (expected root Cargo.toml with members = [\"crates/*\", \"apps/*\"])")]
    NotNestrsWorkspace,

    #[error("feature `{name}` already exists at {path}")]
    FeatureExists { name: String, path: PathBuf },

    #[error("run this command from the nestrs workspace root or pass --path")]
    #[allow(dead_code)]
    WrongDirectory,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

pub type CliResult<T> = Result<T, CliError>;
