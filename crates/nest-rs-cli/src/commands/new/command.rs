//! The `nestrs new` command: infer the layout from the tree and scaffold it
//! through one of the [`standalone`] / [`workspace`] strategies.

//! **One starter, no template flag.** Every layout writes the shared
//! [`hello`](crate::templates::hello) module — a service with a greeting and a
//! `#[public] GET /`. A freshly created project has to prove it started, and a
//! `404` proves nothing to the developer looking at a browser, so there is no
//! routeless variant to pick.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{standalone, workspace};
use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold};
use crate::templates::shared;

#[derive(Debug, Clone)]
pub struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    pub standalone: bool,
    pub dry_run: bool,
}

pub fn run(opts: NewOptions) -> CliResult<()> {
    // Reject a name that would derive an invalid crate identifier (e.g.
    // `"Bad Name!"` → `bad-name!`) before scaffolding a project that won't
    // compile (CLI-I6).
    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);

    if opts.standalone {
        return standalone::scaffold(&opts.output, &names, opts.dry_run);
    }

    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        return workspace::scaffold_app(&ws, &names, opts.dry_run);
    }

    workspace::scaffold_root(&opts.output, &names, opts.dry_run)
}

pub fn project_dir_for_check(opts: &NewOptions, names: &Names) -> CliResult<PathBuf> {
    if opts.standalone {
        return Ok(opts.output.join(&names.kebab));
    }
    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        return Ok(ws.apps_root().join(&names.kebab));
    }
    Ok(opts.output.join(&names.kebab))
}

/// Queue the committed `.env` cascade (`.env`, `.env.development`, `.env.example`).
pub(crate) fn queue_env_files(
    s: &mut Scaffold,
    base: &Path,
    names: &Names,
    env_label: &str,
    env_template: &str,
) {
    let r = Renderer::new(names).with("env_label", env_label);
    s.create_if_missing(base.join(".env"), r.render(env_template));
    s.create_if_missing(
        base.join(".env.development"),
        r.render(shared::ENV_DEVELOPMENT),
    );
    s.create_if_missing(base.join(".env.example"), r.render(shared::ENV_EXAMPLE));
}

pub fn run_cargo_check(project_dir: &Path) -> CliResult<()> {
    let status = Command::new("cargo")
        .arg("check")
        .current_dir(project_dir)
        .status()
        .map_err(CliError::Io)?;
    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo check failed in {}",
            project_dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::templates::{hello, standalone, workspace};

    /// The starter's whole promise: whichever layout renders it, the controller
    /// mounts `/` and declares its posture. A template that stopped emitting
    /// either would ship a project answering 404 on its first page.
    #[test]
    fn the_shared_hello_controller_mounts_root_as_public() {
        assert!(hello::CONTROLLER.contains(r#"#[controller(path = "/")]"#));
        assert!(hello::CONTROLLER.contains(r#"#[get("/")]"#));
        assert!(hello::CONTROLLER.contains("#[public]"));
    }

    /// Both layouts must actually reach it — the standalone crate through its
    /// `providers` list, a workspace app through the feature's HTTP module.
    #[test]
    fn both_layouts_wire_the_hello_controller_in() {
        assert!(standalone::MODULE.contains("providers = [{{service}}, {{controller}}]"));
        assert!(workspace::APP_MODULE.contains("{{http_module}},"));
        assert!(hello::FEATURE_HTTP_MODULE.contains("providers = [{{controller}}]"));
    }
}
