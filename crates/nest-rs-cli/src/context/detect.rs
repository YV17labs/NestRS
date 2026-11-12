//! Structure-based context detection: the [`Context`] the generators read to
//! decide what to auto-wire, and the [`TargetApp`] they wire it into.

use std::path::{Path, PathBuf};

use super::workspace::NestrsWorkspace;
use crate::error::{CliError, CliResult};

/// The app a generator writes the imports its code needs into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetApp {
    /// `apps/<name>/`, its crate root at `root`.
    App { name: String, root: PathBuf },
    /// None: the workspace has no app yet.
    NoApp,
    /// None: the workspace has several apps and nothing chose one — their
    /// names, sorted, for the run to list.
    Unchosen(Vec<String>),
}

#[derive(Debug, Clone)]
pub(crate) struct Context {
    pub workspace: Option<NestrsWorkspace>,
    /// Crate root of the app the cursor is in (`apps/<x>/`), when applicable.
    pub current_app: Option<PathBuf>,
}

impl Context {
    pub(crate) fn detect(start: &Path) -> CliResult<Self> {
        let abs = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        let workspace = NestrsWorkspace::discover(start)?;
        let current_app = workspace.as_ref().and_then(|ws| detect_app(ws, &abs));
        Ok(Self {
            workspace,
            current_app,
        })
    }

    /// The app a generator wires into: the one `named` (`--app`), else the
    /// one the cursor is in, else the workspace's only app. A name no app
    /// answers to is refused, listing the apps.
    pub(crate) fn target_app(&self, named: Option<&str>) -> CliResult<TargetApp> {
        let Some(ws) = &self.workspace else {
            return Ok(TargetApp::NoApp);
        };
        let apps = ws.app_names()?;
        let app = |name: &str| TargetApp::App {
            name: name.to_owned(),
            root: ws.apps_root().join(name),
        };
        if let Some(name) = named {
            return if apps.iter().any(|listed| listed == name) {
                Ok(app(name))
            } else {
                Err(CliError::AppNotFound {
                    name: name.to_owned(),
                    apps: listed(&apps),
                })
            };
        }
        if let Some(name) = self
            .current_app
            .as_ref()
            .and_then(|root| root.file_name())
            .and_then(|name| name.to_str())
        {
            return Ok(app(name));
        }
        Ok(match apps.as_slice() {
            [] => TargetApp::NoApp,
            [only] => app(only),
            _ => TargetApp::Unchosen(apps),
        })
    }
}

/// `apps` as a sentence lists them, or `none` for an empty workspace.
fn listed(apps: &[String]) -> String {
    if apps.is_empty() {
        "none".to_owned()
    } else {
        apps.join(", ")
    }
}

/// `apps/<x>/` containing `dir` → that app's crate root.
fn detect_app(ws: &NestrsWorkspace, dir: &Path) -> Option<PathBuf> {
    let apps = ws.apps_root().canonicalize().ok()?;
    let rest = dir.strip_prefix(&apps).ok()?;
    let app = rest.components().next()?;
    Some(apps.join(app.as_os_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(apps: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\", \"apps/*\"]\n",
        )
        .expect("the root manifest");
        for app in apps {
            let root = dir.path().join("apps").join(app);
            std::fs::create_dir_all(root.join("src")).expect("the app's directory");
            std::fs::write(root.join("Cargo.toml"), "[package]\n").expect("its manifest");
        }
        dir
    }

    fn named(target: TargetApp) -> Option<String> {
        match target {
            TargetApp::App { name, .. } => Some(name),
            TargetApp::NoApp | TargetApp::Unchosen(_) => None,
        }
    }

    #[test]
    fn the_root_of_a_one_app_workspace_targets_that_app() {
        let dir = workspace(&["blog"]);
        let ctx = Context::detect(dir.path()).expect("a workspace");
        assert_eq!(
            named(ctx.target_app(None).expect("resolved")).as_deref(),
            Some("blog")
        );
    }

    #[test]
    fn the_root_of_a_two_app_workspace_targets_none_and_lists_them() {
        let dir = workspace(&["hello", "blog"]);
        let ctx = Context::detect(dir.path()).expect("a workspace");
        assert_eq!(
            ctx.target_app(None).expect("resolved"),
            TargetApp::Unchosen(vec!["blog".to_owned(), "hello".to_owned()])
        );
    }

    #[test]
    fn a_named_app_beats_the_one_the_cursor_is_in() {
        let dir = workspace(&["hello", "blog"]);
        let ctx = Context::detect(&dir.path().join("apps/hello/src")).expect("a workspace");
        assert_eq!(
            named(ctx.target_app(None).expect("resolved")).as_deref(),
            Some("hello")
        );
        assert_eq!(
            named(ctx.target_app(Some("blog")).expect("resolved")).as_deref(),
            Some("blog")
        );
    }

    #[test]
    fn a_name_no_app_answers_to_is_refused_listing_the_apps() {
        let dir = workspace(&["hello", "blog"]);
        let ctx = Context::detect(dir.path()).expect("a workspace");
        let refused = ctx.target_app(Some("../blog")).expect_err("no such app");
        assert_eq!(
            refused.to_string(),
            "no app `../blog` in this workspace — its apps: blog, hello"
        );
    }

    #[test]
    fn an_empty_workspace_targets_no_app() {
        let dir = workspace(&[]);
        let ctx = Context::detect(dir.path()).expect("a workspace");
        assert_eq!(ctx.target_app(None).expect("resolved"), TargetApp::NoApp);
    }
}
