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

/// The framework's own env-var prefix, and what a project gets unless it
/// declares another. Kept as a literal rather than borrowed from
/// `nest-rs-core`: the CLI depends on no framework crate, so that
/// `cargo install nest-rs-cli` stays independent of the version a project pins.
pub const DEFAULT_ENV_PREFIX: &str = "NESTRS";

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
    /// The prefix every framework env var carries in this project — `NESTRS`
    /// unless `nestrs new --env-prefix` set another.
    ///
    /// The runtime learns it from `env_prefix!` in the project's source; this
    /// entry is how *tooling* learns the same fact, since a generator writing
    /// `NESTRS_AUTHN__SECRET` into an `ACME` project's `.env` would emit a key
    /// the app never reads. `nestrs new` writes both.
    pub env_prefix: String,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            port_base: DEFAULT_PORT_BASE,
            env_prefix: DEFAULT_ENV_PREFIX.to_owned(),
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

/// Read `[<container>.metadata.nestrs]` — `container` being the `workspace`
/// table of a monorepo root or the `package` table of a standalone crate, which
/// is why this takes the containing table rather than the document.
fn read_metadata(container: &toml_edit::Table) -> Metadata {
    let mut meta = Metadata::default();
    let Some(table) = container
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
    // A malformed value keeps the default rather than propagating a prefix no
    // `env_prefix!` could have declared — the shape is checked where it is
    // written (`nestrs new`), and `doctor` reports what it resolved.
    if let Some(prefix) = table.get("env-prefix").and_then(|v| v.as_str())
        && validate_env_prefix(prefix).is_ok()
    {
        meta.env_prefix = prefix.to_owned();
    }
    meta
}

/// The env prefix a **standalone** crate at `dir` declares in
/// `[package.metadata.nestrs]`, defaulting to `NESTRS`.
///
/// Inside a workspace, read `NestrsWorkspace::metadata` instead — this is the
/// fallback for the layout `discover` cannot answer for. Never fails: a project
/// may legitimately have no manifest here (`nestrs doctor` runs anywhere), and
/// the default is then the honest answer.
pub fn package_env_prefix(dir: &Path) -> String {
    let Ok(source) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
        return DEFAULT_ENV_PREFIX.to_owned();
    };
    let Ok(doc) = source.parse::<DocumentMut>() else {
        return DEFAULT_ENV_PREFIX.to_owned();
    };
    doc.get("package")
        .and_then(Item::as_table)
        .map(read_metadata)
        .unwrap_or_default()
        .env_prefix
}

/// A framework variable's full name, `<PREFIX>_<NAMESPACE>__<KEY>` — the CLI's
/// mirror of `nest_rs_config::var_name`, since it links no framework crate.
///
/// One function so the name a command *checks* and the name it *prints* cannot
/// be two independent `format!`s that drift.
pub fn var_name(env_prefix: &str, namespace: &str, key: &str) -> String {
    format!("{env_prefix}_{namespace}__{key}")
}

/// The shape `env_prefix!` accepts, restated for the CLI (which links no
/// framework crate). Uppercase ASCII, digits and underscores, starting with a
/// letter and not ending in `_` — the framework supplies the separator.
pub fn validate_env_prefix(prefix: &str) -> Result<(), String> {
    let valid = !prefix.is_empty()
        && prefix.starts_with(|c: char| c.is_ascii_uppercase())
        && !prefix.ends_with('_')
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "`{prefix}` is not a usable env prefix: use uppercase ASCII letters, digits and \
             underscores, starting with a letter and not ending in `_` (e.g. `ACME`, which \
             yields ACME_ENV and ACME_DATABASE__URL)"
        ))
    }
}
