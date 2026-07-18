//! Discovery of the nestrs workspace root.
//!
//! A nestrs monorepo is identified purely by its root `Cargo.toml`
//! (`members = ["crates/*", "apps/*"]`) — no dedicated config file. Optional
//! overrides live in `[workspace.metadata.nestrs]`, read by [`Metadata`].

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::error::{CliError, CliResult};

const NESTRS_WORKSPACE_MARKERS: &[&str] = &["crates/*", "apps/*"];

/// Default HTTP port handed to the first app in a fresh workspace.
pub const DEFAULT_PORT_BASE: u16 = 3000;

#[derive(Debug, Clone)]
pub struct NestrsWorkspace {
    pub root: PathBuf,
    pub metadata: Metadata,
}

/// Opt-in `[workspace.metadata.nestrs]` overrides. Every field has a default,
/// so the table is never required.
#[derive(Debug, Clone)]
pub struct Metadata {
    /// Base port for app port allocation.
    pub port_base: u16,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            port_base: DEFAULT_PORT_BASE,
        }
    }
}

impl NestrsWorkspace {
    pub fn discover(start: &Path) -> CliResult<Option<Self>> {
        let mut dir = start.canonicalize().map_err(CliError::Io)?;
        loop {
            if let Some(ws) = read_workspace(&dir)? {
                return Ok(Some(ws));
            }
            if !dir.pop() {
                return Ok(None);
            }
        }
    }

    pub fn require(start: &Path) -> CliResult<Self> {
        Self::discover(start)?.ok_or(CliError::NotNestrsWorkspace)
    }

    pub fn features_root(&self) -> PathBuf {
        self.root.join("crates/features/src")
    }

    pub fn features_lib(&self) -> PathBuf {
        self.root.join("crates/features/src/lib.rs")
    }

    pub fn features_cargo(&self) -> PathBuf {
        self.root.join("crates/features/Cargo.toml")
    }

    pub fn apps_root(&self) -> PathBuf {
        self.root.join("apps")
    }

    /// `crates/migrations/src` — the SeaORM migration crate's source dir.
    pub fn migrations_root(&self) -> PathBuf {
        self.root.join("crates/migrations/src")
    }

    /// The migration crate's `lib.rs` (the `mod m…;` registry).
    pub fn migrations_lib(&self) -> PathBuf {
        self.migrations_root().join("lib.rs")
    }

    /// The migration crate's `migrator.rs` (the `MigratorTrait` vec — regenerated
    /// from the `lib.rs` module list so both registrations always agree).
    pub fn migrations_migrator(&self) -> PathBuf {
        self.migrations_root().join("migrator.rs")
    }

    pub fn feature_root(&self, snake: &str) -> PathBuf {
        self.features_root().join(snake)
    }

    pub fn feature_exists(&self, snake: &str) -> bool {
        self.feature_root(snake).is_dir()
    }
}

fn read_workspace(dir: &Path) -> CliResult<Option<NestrsWorkspace>> {
    let manifest = dir.join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(None);
    }

    let source = std::fs::read_to_string(&manifest).map_err(CliError::Io)?;
    let doc = source
        .parse::<DocumentMut>()
        .map_err(|e| CliError::Anyhow(e.into()))?;

    let Some(workspace) = doc.get("workspace").and_then(Item::as_table) else {
        return Ok(None);
    };

    let Some(members) = workspace.get("members").and_then(Item::as_array) else {
        return Ok(None);
    };

    let member_strings: Vec<String> = members
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();

    let is_nestrs = NESTRS_WORKSPACE_MARKERS
        .iter()
        .all(|marker| member_strings.iter().any(|member| member == marker));

    if !is_nestrs {
        return Ok(None);
    }

    let metadata = read_metadata(workspace);

    Ok(Some(NestrsWorkspace {
        root: dir.to_path_buf(),
        metadata,
    }))
}

fn read_metadata(workspace: &toml_edit::Table) -> Metadata {
    let mut meta = Metadata::default();
    let Some(table) = workspace
        .get("metadata")
        .and_then(Item::as_table)
        .and_then(|m| m.get("nestrs"))
        .and_then(Item::as_table)
    else {
        return meta;
    };

    if let Some(port) = table.get("port-base").and_then(|v| v.as_integer())
        && let Ok(port) = u16::try_from(port)
    {
        meta.port_base = port;
    }
    meta
}
