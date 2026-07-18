use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::commands::{self, NewTemplate};
use crate::error::CliResult;
use crate::naming::{Names, Transport};

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
                  Scaffolds NestRS projects, features, transport adapters, and toolchain checks.",
    disable_version_flag = true,
    after_help = AFTER_HELP,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new NestRS project or workspace app.
    ///
    /// Layout is inferred from the directory tree:
    ///   new monorepo       nestrs new hello       → ./hello/ (template: hello)
    ///   new workspace app  nestrs new blog        → apps/blog/ (next free port)
    ///   single crate       nestrs new hello --standalone
    New {
        /// Project name (kebab-case recommended, e.g. `hello` or `blog`).
        name: String,

        /// Single-crate layout (logic in `src/`) instead of the default monorepo.
        #[arg(long)]
        standalone: bool,

        /// Parent directory (default: current directory).
        #[arg(long, short = 'o', default_value = ".")]
        output: PathBuf,

        /// Override the starter template (`hello` or `empty`).
        #[arg(long, value_enum)]
        template: Option<NewTemplate>,

        /// Run `cargo check` after scaffolding.
        #[arg(long)]
        check: bool,

        /// Print what would be written without touching the filesystem.
        #[arg(long)]
        dry_run: bool,
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

    /// Install the latest nestrs CLI from crates.io when a newer version exists.
    Update {
        /// Reinstall from `crates/nest-rs-cli` in the nestrs monorepo instead of crates.io.
        #[arg(long)]
        from_path: bool,

        /// Monorepo root when using `--from-path` (default: auto-discover).
        #[arg(long, requires = "from_path")]
        workspace: Option<PathBuf>,

        /// Reinstall even when already on the latest version (`cargo install --force`).
        #[arg(long, short = 'f')]
        force: bool,
    },

    /// Generate features, resources, and transport adapters (workspace only).
    #[command(subcommand, visible_aliases = ["g"])]
    Generate(GenerateCommand),

    /// Run a project task through `just` (bootstraps the dev toolchain on first use).
    ///
    /// Forwards the recipe and its arguments verbatim:
    ///   nestrs run dev      → just dev
    ///   nestrs run test     → just test
    ///   nestrs run db up    → just db up
    ///   nestrs run          → list available recipes
    Run {
        /// Skip the first-run toolchain bootstrap (CI / offline).
        #[arg(long)]
        no_bootstrap: bool,

        /// Recipe and arguments forwarded to `just` (omit to list recipes).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Shared positional + flags for every generator.
#[derive(Args, Debug)]
pub struct GenTarget {
    /// Name (e.g. `posts`).
    pub name: String,

    /// Workspace root or working directory (default: auto-discover from cwd).
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Print what would be written without touching the filesystem.
    #[arg(long)]
    pub dry_run: bool,
}

/// `g resource` target: the shared flags plus the guarded-form opt-in.
#[derive(Args, Debug)]
pub struct ResourceTarget {
    #[command(flatten)]
    pub target: GenTarget,

    /// Scaffold the hardened `#[crud]` + guards form instead of the
    /// unguarded stub. Requires the workspace to provide `AuthGuard`,
    /// `AuthzGuard`, and `AuthzHttpModule` (as the `demo` does).
    #[arg(long)]
    pub guarded: bool,
}

#[derive(Subcommand, Debug)]
pub enum GenerateCommand {
    /// A transport-agnostic port (mod + module + service).
    Feature(GenTarget),
    /// A DB-backed CRUD slice (entity + CrudService + HTTP adapter).
    Resource(ResourceTarget),
    /// Add an HTTP controller adapter to an existing feature.
    Http(GenTarget),
    /// Add a GraphQL resolver adapter to an existing feature.
    Graphql(GenTarget),
    /// Add a WebSocket gateway adapter to an existing feature.
    Ws(GenTarget),
    /// Add a queue processor adapter to an existing feature.
    Queue(GenTarget),
    /// Add a scheduled-tasks adapter to an existing feature.
    Schedule(GenTarget),
    /// Add an MCP tool adapter to an existing feature.
    Mcp(GenTarget),
}

pub fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::New {
            name,
            standalone,
            output,
            template,
            check,
            dry_run,
        } => {
            let names = Names::parse(&name);
            let opts = commands::NewOptions {
                name,
                output: output.clone(),
                template,
                standalone,
                dry_run,
            };
            commands::run_new(opts.clone())?;
            if check && !dry_run {
                run_check(&opts, &names, standalone, &output)?;
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
        Command::Update {
            from_path,
            workspace,
            force,
        } => commands::run_update(commands::UpdateOptions {
            from_path,
            path: workspace,
            force,
        }),
        Command::Generate(cmd) => run_generate(cmd),
        Command::Run { no_bootstrap, args } => {
            commands::run_task(commands::RunOptions { args, no_bootstrap })
        }
    }
}

fn run_generate(cmd: GenerateCommand) -> CliResult<()> {
    use GenerateCommand::*;
    match cmd {
        Feature(t) => commands::run_feature(commands::FeatureOptions {
            name: t.name,
            path: t.path,
            dry_run: t.dry_run,
        }),
        Resource(t) => commands::run_resource(commands::ResourceOptions {
            name: t.target.name,
            path: t.target.path,
            dry_run: t.target.dry_run,
            guarded: t.guarded,
        }),
        Http(t) => adapter(Transport::Http, t),
        Graphql(t) => adapter(Transport::Graphql, t),
        Ws(t) => adapter(Transport::Ws, t),
        Queue(t) => adapter(Transport::Queue, t),
        Schedule(t) => adapter(Transport::Schedule, t),
        Mcp(t) => adapter(Transport::Mcp, t),
    }
}

fn adapter(transport: Transport, t: GenTarget) -> CliResult<()> {
    commands::run_adapter(
        transport,
        commands::AdapterOptions {
            name: t.name,
            path: t.path,
            dry_run: t.dry_run,
        },
    )
}

fn run_check(
    opts: &commands::NewOptions,
    names: &Names,
    standalone: bool,
    output: &std::path::Path,
) -> CliResult<()> {
    let project = commands::project_dir_for_check(opts, names)?;
    let ws_root = crate::context::NestrsWorkspace::discover(output)?;
    if let Some(ws) = ws_root.filter(|_| !standalone) {
        let status = std::process::Command::new("cargo")
            .args(["check", "-p", &names.kebab])
            .current_dir(&ws.root)
            .status()
            .map_err(crate::error::CliError::Io)?;
        if !status.success() {
            return Err(crate::error::CliError::Anyhow(anyhow::anyhow!(
                "cargo check -p {} failed",
                names.kebab
            )));
        }
    } else {
        commands::run_cargo_check(&project)?;
    }
    println!("cargo check passed.");
    Ok(())
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
