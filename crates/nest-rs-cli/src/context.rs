use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::error::{CliError, CliResult};

const NESTRS_WORKSPACE_MARKERS: &[&str] = &["crates/*", "apps/*"];

#[derive(Debug, Clone)]
pub struct NestrsWorkspace {
    pub root: PathBuf,
    #[allow(dead_code)]
    pub version: String,
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
        Self::discover(start)?
            .ok_or(CliError::NotNestrsWorkspace)
    }

    pub fn features_root(&self) -> PathBuf {
        self.root.join("crates/features/src")
    }

    pub fn features_lib(&self) -> PathBuf {
        self.root.join("crates/features/src/lib.rs")
    }

    pub fn apps_root(&self) -> PathBuf {
        self.root.join("apps")
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

    let version = workspace
        .get("package")
        .and_then(Item::as_table)
        .and_then(|pkg| pkg.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_owned();

    Ok(Some(NestrsWorkspace {
        root: dir.to_path_buf(),
        version,
    }))
}
