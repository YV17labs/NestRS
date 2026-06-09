use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("path already exists: {}", .0.display())]
    AlreadyExists(PathBuf),

    #[error("app `{name}` already exists at {path}")]
    AppExists { name: String, path: PathBuf },

    #[error(
        "not inside a nestrs workspace (expected root Cargo.toml with members = [\"crates/*\", \"apps/*\"])"
    )]
    NotNestrsWorkspace,

    #[error("feature `{name}` already exists at {path}")]
    FeatureExists { name: String, path: PathBuf },

    #[error("feature `{name}` not found — create it first with `nestrs g feature {name}`")]
    FeatureNotFound { name: String },

    #[error("{0}")]
    InvalidFeatureName(String),

    #[error("{transport} adapter for `{name}` already exists at {path}")]
    AdapterExists {
        transport: &'static str,
        name: String,
        path: PathBuf,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

pub type CliResult<T> = Result<T, CliError>;
