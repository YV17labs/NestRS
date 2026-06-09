use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::commands::{self, NewTemplate};
use crate::error::CliResult;
use crate::naming::Names;

const PROJECT_TAGLINE: &str = "Scalable Rust backend apps with native performance.";

const AFTER_HELP: &str = concat!(
    "Documentation: ",
    env!("CARGO_PKG_HOMEPAGE"),
    "/cli/\n",
    "Repository:    ",
    env!("CARGO_PKG_REPOSITORY"),
);

pub fn print_version() {
    println!("NestRS {}", env!("CARGO_PKG_VERSION"));
}

pub fn print_about() {
    println!("NestRS");
    println!("Version:       {}", env!("CARGO_PKG_VERSION"));
    println!("Tagline:       {PROJECT_TAGLINE}");
    println!("Documentation: {}/cli/", env!("CARGO_PKG_HOMEPAGE"));
    println!("Repository:    {}", env!("CARGO_PKG_REPOSITORY"));
    println!("License:       {}", env!("CARGO_PKG_LICENSE"));
    println!("Authors:       Yoann Vanitou");
}

#[derive(Parser, Debug)]
#[command(
    name = "nestrs",
    about = PROJECT_TAGLINE,
    long_about = "Scalable Rust backend apps with native performance.\n\n\
                  Scaffolds NestRS projects, features, and toolchain checks.",
    disable_version_flag = true,
    after_help = AFTER_HELP,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new nestrs application.
    New {
        /// Application name (kebab-case recommended, e.g. `my-api`).
        name: String,

        /// Write into `apps/<name>/` inside the nestrs monorepo.
        #[arg(long)]
        in_workspace: bool,

        /// Output directory for standalone projects (default: current directory).
        #[arg(long, short = 'o', default_value = ".")]
        output: PathBuf,

        /// Starter template.
        #[arg(long, value_enum, default_value_t = NewTemplate::Hello)]
        template: NewTemplate,

        /// Run `cargo check` after scaffolding (standalone projects only).
        #[arg(long)]
        check: bool,
    },

    /// Verify toolchain and optional NestRS environment variables.
    Doctor {
        /// Project directory to inspect (default: current directory).
        #[arg(long, short = 'p')]
        path: Option<PathBuf>,
    },

    /// Print the CLI version.
    Version,

    /// Print project metadata (tagline, docs, license, author).
    About,

    /// Reinstall the latest nestrs CLI from crates.io.
    Update {
        /// Reinstall from `crates/nest-rs-cli` in the nestrs monorepo instead of crates.io.
        #[arg(long)]
        from_path: bool,

        /// Monorepo root when using `--from-path` (default: auto-discover).
        #[arg(long, requires = "from_path")]
        workspace: Option<PathBuf>,
    },

    /// Generate code from the reference exemplars.
    #[command(subcommand, visible_aliases = ["g"])]
    Generate(GenerateCommand),
}

#[derive(Subcommand, Debug)]
pub enum GenerateCommand {
    /// Scaffold a port feature under `crates/features/src/` (monorepo only).
    Feature {
        /// Feature name (e.g. `posts`).
        name: String,

        /// Also emit the HTTP adapter (`http/controller.rs`, `UsersHttpModule` pattern).
        #[arg(long)]
        http: bool,

        /// Workspace root (default: auto-discover from current directory).
        #[arg(long, short = 'p')]
        path: Option<PathBuf>,
    },
}

pub fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::New {
            name,
            in_workspace,
            output,
            template,
            check,
        } => {
            let names = Names::parse(&name);
            commands::run_new(commands::NewOptions {
                name,
                output: output.clone(),
                template,
                in_workspace,
            })?;

            if check && !in_workspace {
                let project = output.join(&names.kebab);
                commands::run_cargo_check(&project)?;
                println!("cargo check passed.");
            }
            Ok(())
        }
        Command::Doctor { path } => {
            commands::run_doctor(commands::DoctorOptions { path })?;
            Ok(())
        }
        Command::Version => {
            print_version();
            Ok(())
        }
        Command::About => {
            print_about();
            Ok(())
        }
        Command::Update { from_path, workspace } => commands::run_update(commands::UpdateOptions {
            from_path,
            path: workspace,
        }),
        Command::Generate(GenerateCommand::Feature { name, http, path }) => {
            commands::run_feature(commands::FeatureOptions {
                name,
                http,
                path,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_help_includes_docs_and_repo() {
        assert!(AFTER_HELP.contains("/cli/"));
        assert!(AFTER_HELP.contains("github.com"));
    }
}
