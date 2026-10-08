//! The `nestrs new` command: infer the layout from the tree and scaffold it
//! through the [`workspace`] strategy — a fresh monorepo, or an app added to
//! one that already exists.

//! **One starter, no template flag.** Every layout writes the shared
//! [`hello`](crate::templates::hello) module — a service with a greeting and a
//! `#[public] GET /`, so a fresh project proves it started.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::workspace;
use crate::context::{DEFAULT_ENV_PREFIX, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold};
use crate::templates::shared;

#[derive(Debug, Clone)]
pub(crate) struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    /// `None` ⇒ the framework default (`NESTRS`).
    pub env_prefix: Option<String>,
    pub dry_run: bool,
}

pub(crate) fn run(opts: NewOptions) -> CliResult<()> {
    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);

    if let Some(prefix) = &opts.env_prefix {
        crate::context::validate_env_prefix(prefix)
            .map_err(|e| CliError::Anyhow(anyhow::anyhow!(e)))?;
    }
    let env_prefix = opts.env_prefix.as_deref().unwrap_or(DEFAULT_ENV_PREFIX);

    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        // The prefix belongs to the deployment: an app added to an existing project
        // inherits it, and ignoring the flag silently would mislead the caller.
        if opts.env_prefix.is_some() {
            return Err(CliError::Anyhow(anyhow::anyhow!(
                "`--env-prefix` applies to project creation only — an app added to an \
                 existing workspace inherits the project's prefix. Set `{}` in the \
                 environment that runs it (the `Justfile`, your container, your shell).",
                crate::context::ENV_PREFIX_VAR,
            )));
        }
        return workspace::scaffold_app(&ws, &names, opts.dry_run);
    }

    workspace::scaffold_root(&opts.output, &names, env_prefix, opts.dry_run)
}

/// Seed every prefix placeholder — the value templates interpolate into
/// variable names, and the two lines that *set* it for the processes this
/// project starts.
/// Both setters are empty on the default. The `.env` cascade does **not** carry
/// it: the prefix selects the cascade, and the framework aborts on one inside it.
pub(crate) fn with_env_prefix(r: Renderer, env_prefix: &str) -> Renderer {
    prefix_vars(env_prefix)
        .into_iter()
        .fold(r, |r, (key, value)| r.with(key, value))
}

/// Every renderer key whose value depends on the project's env prefix, in one
/// list so the default seed (`Renderer::new`) and the `--env-prefix` override
/// cannot disagree. The note is rendered here because substitution is one pass:
/// a raw `{{env_prefix}}` inside a seeded value would survive it.
pub(crate) fn prefix_vars(env_prefix: &str) -> Vec<(&'static str, String)> {
    // `{{env_prefix_var}}` does not match `{{env_prefix}}`, so order is irrelevant.
    let fill = |template: &str| {
        template
            .replace("{{env_prefix_var}}", crate::context::ENV_PREFIX_VAR)
            .replace("{{env_prefix}}", env_prefix)
    };
    let default = env_prefix == DEFAULT_ENV_PREFIX;
    vec![
        ("env_prefix", env_prefix.to_owned()),
        ("dev_recipe_note", fill(shared::DEV_RECIPE_NOTE)),
        (
            "env_prefix_export",
            if default {
                String::new()
            } else {
                fill(shared::ENV_PREFIX_JUSTFILE)
            },
        ),
    ]
}

/// Queue the committed `.env` cascade (`.env`, `.env.development`, `.env.example`).
///
/// Every key in those files is written through `{{env_prefix}}`, so a project
/// created with `--env-prefix` gets a cascade its app actually reads.
pub(crate) fn queue_env_files(s: &mut Scaffold, base: &Path, r: &Renderer) {
    s.create_if_missing(base.join(".env"), r.render(shared::ENV));
    s.create_if_missing(
        base.join(".env.development"),
        r.render(shared::ENV_DEVELOPMENT),
    );
    s.create_if_missing(base.join(".env.example"), r.render(shared::ENV_EXAMPLE));
}

/// Queue the two files that carry the project's conventions: `AGENTS.md`, the
/// format every coding agent reads, and a `CLAUDE.md` that imports it (Claude
/// Code reads only the latter).
///
/// Assembled as `INTRO + LAYOUT + BODY`; the body embeds the architecture rules
/// verbatim.
pub(crate) fn queue_agent_files(s: &mut Scaffold, base: &Path, r: &Renderer) {
    let body = format!(
        "{}{}{}",
        shared::AGENTS_INTRO,
        shared::AGENTS_LAYOUT,
        shared::AGENTS_BODY
    );
    s.create(base.join("AGENTS.md"), r.render(&body));
    s.create(base.join("CLAUDE.md"), r.render(shared::CLAUDE_POINTER));
}

pub(crate) fn run_cargo_check(project_dir: &Path) -> CliResult<()> {
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
    use crate::templates::{hello, workspace};

    /// Whichever layout renders it, the controller mounts `/` and declares its
    /// posture.
    #[test]
    fn the_shared_hello_controller_mounts_root_as_public() {
        assert!(hello::CONTROLLER.contains(r#"#[controller(path = "/")]"#));
        assert!(hello::CONTROLLER.contains(r#"#[get("/")]"#));
        assert!(hello::CONTROLLER.contains("#[public]"));
    }

    /// The app must actually reach it: the app module imports the feature's
    /// HTTP module, and that module lists the controller as a provider.
    #[test]
    fn the_app_wires_the_hello_controller_in() {
        assert!(workspace::APP_MODULE.contains("{{http_module}},"));
        assert!(hello::FEATURE_HTTP_MODULE.contains("providers = [{{controller}}]"));
    }
}
