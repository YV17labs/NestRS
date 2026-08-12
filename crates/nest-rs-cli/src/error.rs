use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
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

    /// `g entity` on a feature whose entity still lives in `entity.rs`.
    ///
    /// One role, one file per folder: a module keeps either the lone
    /// `entity.rs` or an `entities/` directory, never both. The move is left to
    /// the developer on purpose — see the refusal's reasoning in
    /// `commands::generate::entity`.
    #[error(
        "feature `{feature}` keeps its entity in `entity.rs`, and a module holds either one \
         `entity.rs` or an `entities/` folder — never both. Move the existing one first, then \
         re-run:\n  \
         1. mkdir crates/features/src/{feature}/entities && git mv \
         crates/features/src/{feature}/entity.rs crates/features/src/{feature}/entities/{stem}.rs\n  \
         2. write `entities/mod.rs` with `pub mod {stem};`\n  \
         3. in `{feature}/mod.rs`, replace `mod entity;` / `pub use entity::*;` with \
         `mod entities;` / `pub use entities::{stem}::*;`\n  \
         4. deepen by one level every `super::` path the moved file carries, and every \
         `super::entity::` path elsewhere in the feature that names it"
    )]
    EntitiesFolderRequired { feature: String, stem: String },

    #[error(
        "`{0}` is not a `<feature>` or `<feature>/<entity>` target — one optional `/`, no more"
    )]
    InvalidEntityTarget(String),

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
