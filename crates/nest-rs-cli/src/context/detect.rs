//! Structure-based context detection: the [`Context`] the generators read to
//! decide what to auto-wire.

use std::path::{Path, PathBuf};

use super::workspace::NestrsWorkspace;
use crate::error::CliResult;

#[derive(Debug, Clone)]
pub struct Context {
    pub workspace: Option<NestrsWorkspace>,
    /// Crate root of the app the cursor is in (`apps/<x>/`), when applicable.
    pub current_app: Option<PathBuf>,
}

impl Context {
    pub fn detect(start: &Path) -> CliResult<Self> {
        let abs = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        let workspace = NestrsWorkspace::discover(start)?;
        let current_app = workspace.as_ref().and_then(|ws| detect_app(ws, &abs));
        Ok(Self {
            workspace,
            current_app,
        })
    }

    /// The `module.rs` of the app the cursor is in, if any.
    pub fn current_app_module(&self) -> Option<PathBuf> {
        self.current_app
            .as_ref()
            .map(|app| app.join("src/module.rs"))
    }
}

/// `apps/<x>/` containing `dir` → that app's crate root.
fn detect_app(ws: &NestrsWorkspace, dir: &Path) -> Option<PathBuf> {
    let apps = ws.apps_root().canonicalize().ok()?;
    let rest = dir.strip_prefix(&apps).ok()?;
    let app = rest.components().next()?;
    Some(apps.join(app.as_os_str()))
}
