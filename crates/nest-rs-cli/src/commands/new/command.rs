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
use crate::context::{DEFAULT_ENV_PREFIX, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold};
use crate::templates::shared;

#[derive(Debug, Clone)]
pub struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    pub standalone: bool,
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

    if opts.standalone {
        return standalone::scaffold(&opts.output, &names, env_prefix, opts.dry_run);
    }

    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        // The prefix is a property of the project, declared once at its root:
        // a second app cannot hold a different one, and silently ignoring the
        // flag would leave the caller believing it took.
        if let Some(requested) = &opts.env_prefix
            && requested != &ws.metadata.env_prefix
        {
            return Err(CliError::Anyhow(anyhow::anyhow!(
                "this workspace already uses the `{}` env prefix — `--env-prefix` applies to \
                 project creation only. Change it in the root `Cargo.toml` \
                 (`[workspace.metadata.nestrs] env-prefix`) and in the `env_prefix!` \
                 declaration in `crates/features/src/lib.rs`.",
                ws.metadata.env_prefix,
            )));
        }
        return workspace::scaffold_app(&ws, &names, opts.dry_run);
    }

    workspace::scaffold_root(&opts.output, &names, env_prefix, opts.dry_run)
}

/// The source line that tells the **runtime** which prefix to resolve — empty
/// on the default, so an ordinary project carries no noise about a prefix it
/// never changed.
///
/// One per generated *binary*: `env_prefix!` is a link-time fact, so the crates
/// a binary does not link (the `migrations`/`seed` tools do not link `features`)
/// each need their own. Repeating the same literal is allowed by design.
///
/// Trailing blank line, no leading one: the same value then reads right at the
/// top of a standalone `lib.rs` and under the `//!` of a crate that has one.
pub(crate) fn env_prefix_decl(env_prefix: &str) -> String {
    if env_prefix == DEFAULT_ENV_PREFIX {
        return String::new();
    }
    format!("nest_rs::env_prefix!(\"{env_prefix}\");\n\n")
}

/// Seed both prefix placeholders on a renderer that writes a manifest *and*
/// source: the `[<table>.metadata.nestrs]` entry tooling reads back, and
/// [`env_prefix_decl`]. `table` is `workspace` for a monorepo root, `package`
/// for a standalone crate.
pub(crate) fn with_env_prefix(r: Renderer, env_prefix: &str, table: &str) -> Renderer {
    let metadata = if env_prefix == DEFAULT_ENV_PREFIX {
        String::new()
    } else {
        format!(
            "\n# Every framework env var carries this prefix ({env_prefix}_ENV, \
             {env_prefix}_HTTP__PORT, …).\n\
             # `nestrs` reads it here; the app declares it to the runtime with `env_prefix!`.\n\
             [{table}.metadata.nestrs]\nenv-prefix = \"{env_prefix}\"\n",
        )
    };
    r.with("env_prefix", env_prefix)
        .with("env_prefix_metadata", metadata)
        .with("env_prefix_decl", env_prefix_decl(env_prefix))
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
///
/// Every key in those files is written through `{{env_prefix}}`, so a project
/// created with `--env-prefix` gets a cascade its app actually reads.
pub(crate) fn queue_env_files(
    s: &mut Scaffold,
    base: &Path,
    names: &Names,
    env_label: &str,
    env_prefix: &str,
    env_template: &str,
) {
    let r = Renderer::new(names)
        .with("env_label", env_label)
        .with("env_prefix", env_prefix);
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
