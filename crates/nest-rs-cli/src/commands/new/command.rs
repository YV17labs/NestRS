//! The `nestrs new` command: infer the layout from the tree and scaffold it
//! through one of the [`standalone`](super::standalone) /
//! [`workspace`](super::workspace) strategies.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;

use super::{standalone, workspace};
use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold};
use crate::templates::shared;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum NewTemplate {
    /// Hello World baseline — `GET /`, no auth, no DB (greenfield / standalone).
    #[default]
    Hello,
    /// HTTP transport only — no routes yet.
    Empty,
}

impl NewTemplate {
    pub fn description(self) -> &'static str {
        match self {
            Self::Hello => "hello — Hello World on GET /",
            Self::Empty => "empty — HTTP transport only, no routes",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    /// When `None`, defaults to `hello` for new projects and standalone crates.
    pub template: Option<NewTemplate>,
    pub standalone: bool,
    pub dry_run: bool,
}

pub fn effective_template(opts: &NewOptions) -> NewTemplate {
    opts.template.unwrap_or(NewTemplate::Hello)
}

pub fn run(opts: NewOptions) -> CliResult<()> {
    let names = Names::parse(&opts.name);
    let template = effective_template(&opts);

    if opts.standalone {
        return standalone::scaffold(&opts.output, &names, template, opts.dry_run);
    }

    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        return workspace::scaffold_app(&ws, &names, opts.dry_run);
    }

    workspace::scaffold_root(&opts.output, &names, template, opts.dry_run)
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
    use super::*;

    fn opts(template: Option<NewTemplate>) -> NewOptions {
        NewOptions {
            name: "acme".into(),
            output: PathBuf::from("."),
            template,
            standalone: false,
            dry_run: false,
        }
    }

    #[test]
    fn effective_template_defaults_to_hello() {
        assert_eq!(effective_template(&opts(None)), NewTemplate::Hello);
    }

    #[test]
    fn effective_template_honors_explicit_override() {
        assert_eq!(
            effective_template(&opts(Some(NewTemplate::Empty))),
            NewTemplate::Empty
        );
    }
}
