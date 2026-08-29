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

/// The framework's own env-var prefix, and what a project gets unless the
/// environment names another. Kept as a literal rather than borrowed from
/// `nest-rs-core`: the CLI depends on no framework crate, so that
/// `cargo install nest-rs-cli` stays independent of the version a project pins.
pub const DEFAULT_ENV_PREFIX: &str = "NESTRS";

/// The variable the prefix is read from — the one name no prefix can rename,
/// which is why it is spelled here (`nest_rs_core::EnvPrefix::VAR` is the same
/// literal, for the same reason).
pub const ENV_PREFIX_VAR: &str = "NESTRS_ENV_PREFIX";

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

/// The `nest-rs` version requirement a manifest pins — the framework line the
/// project builds against, which is not necessarily the one this CLI would
/// scaffold ([`crate::version::framework_req`]).
///
/// `None` covers both "no `nest-rs` entry" and an entry carrying no version (a
/// `path` dependency in a contributor's tree): neither is a requirement to
/// report, and inventing one would be worse than saying nothing.
pub fn framework_pin(manifest_dir: &Path) -> CliResult<Option<String>> {
    let Some(doc) = read_manifest(manifest_dir)? else {
        return Ok(None);
    };
    let entry = dependency(&doc, &["workspace", "dependencies"], "nest-rs")
        .or_else(|| dependency(&doc, &["dependencies"], "nest-rs"));
    Ok(entry.and_then(|item| match item.as_str() {
        Some(literal) => Some(literal.to_owned()),
        None => item
            .get("version")
            .and_then(Item::as_str)
            .map(str::to_owned),
    }))
}

/// One dependency entry, from a table reached by `path`.
fn dependency(doc: &DocumentMut, path: &[&str], name: &str) -> Option<Item> {
    let mut table = doc.as_table().get(path.first()?)?;
    for key in &path[1..] {
        table = table.get(key)?;
    }
    table.get(name).cloned()
}

/// A directory's `Cargo.toml`, parsed. `None` when there is none.
fn read_manifest(dir: &Path) -> CliResult<Option<DocumentMut>> {
    let manifest = dir.join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(None);
    }
    let source = std::fs::read_to_string(&manifest).map_err(CliError::Io)?;
    source
        .parse::<DocumentMut>()
        .map(Some)
        .map_err(|e| CliError::Anyhow(e.into()))
}

fn read_workspace(dir: &Path) -> CliResult<Option<NestrsWorkspace>> {
    let Some(doc) = read_manifest(dir)? else {
        return Ok(None);
    };

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

    Ok(Some(NestrsWorkspace {
        root: dir.to_path_buf(),
        metadata: read_metadata(workspace),
    }))
}

/// Read `[workspace.metadata.nestrs]` off the `workspace` table.
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

/// Where the CLI's idea of the prefix comes from — the same environment the app
/// will read, so the two cannot disagree about a project they both see.
///
/// There is deliberately no project file to fall back on: a second source is
/// how a rename half-lands, and `doctor` reporting `ACME` from a manifest while
/// the deployed process resolves `NESTRS` is precisely the failure the single
/// variable exists to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EnvPrefixSource {
    /// Nothing names one; every variable is built from `NESTRS`.
    #[default]
    Unset,
    /// The environment names it, and it is usable.
    Environment(String),
    /// The environment names something the *app* would abort on, so the CLI
    /// must not quietly build names from the default and look correct. Carries
    /// the validator's sentence, which already quotes the offending value.
    Invalid(String),
}

impl EnvPrefixSource {
    /// Read the environment this process was given.
    pub fn detect() -> Self {
        let Ok(value) = std::env::var(ENV_PREFIX_VAR) else {
            return Self::Unset;
        };
        // Empty is unset, the way an empty variable is everywhere else.
        if value.is_empty() {
            return Self::Unset;
        }
        match validate_env_prefix(&value) {
            Ok(()) => Self::Environment(value),
            Err(reason) => Self::Invalid(reason),
        }
    }

    /// The prefix to build names from — the default whenever there is no usable
    /// one, which is also what the app resolves in that case.
    pub fn prefix(&self) -> &str {
        match self {
            Self::Environment(prefix) => prefix,
            Self::Unset | Self::Invalid(_) => DEFAULT_ENV_PREFIX,
        }
    }
}

/// The prefix every name this CLI writes or checks must carry.
pub fn env_prefix() -> String {
    EnvPrefixSource::detect().prefix().to_owned()
}

/// A framework variable's full name, `<PREFIX>_<NAMESPACE>__<KEY>` — the CLI's
/// mirror of `nest_rs_config::var_name`, since it links no framework crate.
///
/// One function so the name a command *checks* and the name it *prints* cannot
/// be two independent `format!`s that drift.
pub fn var_name(env_prefix: &str, namespace: &str, key: &str) -> String {
    format!("{env_prefix}_{namespace}__{key}")
}

/// The shape the runtime accepts, restated for the CLI (which links no
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
             yields ACME_ENV and ACME_SEAORM__URL)"
        ))
    }
}
