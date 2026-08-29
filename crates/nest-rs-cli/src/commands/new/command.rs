//! The `nestrs new` command: infer the layout from the tree and scaffold it
//! through the [`workspace`] strategy — a fresh monorepo, or an app added to
//! one that already exists.

//! **One starter, no template flag.** Every layout writes the shared
//! [`hello`](crate::templates::hello) module — a service with a greeting and a
//! `#[public] GET /`. A freshly created project has to prove it started, and a
//! `404` proves nothing to the developer looking at a browser, so there is no
//! routeless variant to pick.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::workspace;
use crate::context::{DEFAULT_ENV_PREFIX, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold};
use crate::templates::shared;

#[derive(Debug, Clone)]
pub struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    /// `None` ⇒ the framework default (`NESTRS`).
    pub env_prefix: Option<String>,
    pub dry_run: bool,
}

pub fn run(opts: NewOptions) -> CliResult<()> {
    // Reject a name that would derive an invalid crate identifier (e.g.
    // `"Bad Name!"` → `bad-name!`) before scaffolding a project that won't
    // compile (CLI-I6).
    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);

    if let Some(prefix) = &opts.env_prefix {
        crate::context::validate_env_prefix(prefix)
            .map_err(|e| CliError::Anyhow(anyhow::anyhow!(e)))?;
    }
    let env_prefix = opts.env_prefix.as_deref().unwrap_or(DEFAULT_ENV_PREFIX);

    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        // The prefix belongs to the deployment, not to a crate: an app added to
        // an existing project inherits whatever its environment names, and
        // silently ignoring the flag would leave the caller believing it took.
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
///
/// Both setters are empty on the default, so an ordinary project carries no
/// noise about a prefix it never changed. There is no third site and no file in
/// the source tree: the runtime reads the prefix from the environment, so
/// anything a crate said about it would be decoration.
///
/// The `.env` cascade deliberately does **not** carry it. It is read *after* the
/// prefix has already selected which cascade to read, so a value placed there
/// would rename nothing — the framework aborts on it rather than let that pass.
pub(crate) fn with_env_prefix(r: Renderer, env_prefix: &str) -> Renderer {
    prefix_vars(env_prefix)
        .into_iter()
        .fold(r, |r, (key, value)| r.with(key, value))
}

/// Every renderer key whose value depends on the project's env prefix, in one
/// list so the default seed (`Renderer::new`) and the `--env-prefix` override
/// cannot disagree about what the set contains.
///
/// One list rather than two lists plus a test comparing them: a key added to
/// the override alone is a `{{placeholder}}` written verbatim into whatever the
/// other paths render, and nothing in a compile or a scaffold would say so.
///
/// The two setter lines are empty on the default — a project on `NESTRS` sets
/// nothing — and the note is rendered here because substitution is one pass, so
/// a raw `{{env_prefix}}` inside a seeded value would survive it.
pub(crate) fn prefix_vars(env_prefix: &str) -> Vec<(&'static str, String)> {
    // Both keys these two templates carry. `{{env_prefix_var}}` does not match
    // `{{env_prefix}}` (the closing braces differ), so order is irrelevant —
    // but leaving it out ships the placeholder, which is what this list exists
    // to make impossible.
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
/// The document is `INTRO + LAYOUT + BODY`, assembled here so the three pieces
/// stay separately editable — the middle one is the layout a project is handed,
/// and the last embeds the architecture rules verbatim.
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
    use crate::templates::{hello, workspace};

    /// The starter's whole promise: whichever layout renders it, the controller
    /// mounts `/` and declares its posture. A template that stopped emitting
    /// either would ship a project answering 404 on its first page.
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
