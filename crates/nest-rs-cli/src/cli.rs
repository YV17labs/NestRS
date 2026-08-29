use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::commands;
use crate::error::CliResult;
use crate::naming::{Names, Transport};

const PROJECT_TAGLINE: &str = "The Rust framework for modular, scalable backends.";

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
    long_about = "The Rust framework for modular, scalable backends — you write the \
                  business logic, it carries the rest.\n\n\
                  Scaffolds NestRS projects, features, transport adapters, and toolchain checks.",
    // `--version` / `-V` are intercepted in `main` so they print exactly what
    // `nestrs version` prints, rather than clap's `<bin> <ver>` rendering.
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
    /// Every layout ships the same `hello` module — a `#[public] GET /` that
    /// proves the project started. Layout is inferred from the directory tree:
    ///   new monorepo       nestrs new hello       → ./hello/ + apps/hello/
    ///   new workspace app  nestrs new blog        → apps/blog/ (next free port)
    #[command(verbatim_doc_comment)]
    New {
        /// Project name (kebab-case recommended, e.g. `hello` or `blog`).
        name: String,

        /// Parent directory (default: current directory).
        #[arg(long, short = 'o', default_value = ".")]
        output: PathBuf,

        /// Prefix every framework env var carries, instead of `NESTRS`
        /// (e.g. `ACME` ⇒ `ACME_ENV`, `ACME_HTTP__PORT`). Uppercase ASCII.
        /// Written into the Justfile, which sets it on the processes it
        /// starts; your deployment must set it too.
        #[arg(long, value_name = "PREFIX")]
        env_prefix: Option<String>,

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

    /// Check that every file is named for what it declares.
    ///
    /// One shape is refused: a file whose stem reaches nothing it declares.
    /// That file was named for a slot rather than a subject, and a slot fills.
    #[command(verbatim_doc_comment)]
    Lint {
        /// Project directory to inspect (default: current directory).
        #[arg(long, short = 'p')]
        path: Option<PathBuf>,
    },

    /// Print the CLI version.
    Version,

    /// Print NestRS metadata (tagline, docs, license, author).
    About,

    /// Report the project the current directory sits in.
    ///
    /// `about` is the framework; `info` is your tree — layout, root, apps,
    /// features, the framework version the manifests pin, the env prefix in
    /// force, and the toolchain. Outside a project it says so rather than
    /// failing.
    Info {
        /// Project directory to inspect (default: current directory).
        #[arg(long, short = 'p')]
        path: Option<PathBuf>,
    },

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
    #[command(verbatim_doc_comment)]
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

/// `g entity` takes a two-part target: the feature it joins, and — when the
/// feature's own singular is not the entity's name — the entity itself. A
/// struct of its own rather than [`GenTarget`] so `--help` spells that grammar
/// at the positional, where the reader is looking.
#[derive(Args, Debug)]
pub struct EntityTarget {
    /// Feature the entity joins, optionally with its own name:
    /// `posts` (⇒ `Post`) or `posts/comment` (⇒ `Comment`).
    pub target: String,

    /// Workspace root or working directory (default: auto-discover from cwd).
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Print what would be written without touching the filesystem.
    #[arg(long)]
    pub dry_run: bool,
}

/// `g auth` takes no name — a workspace has exactly one auth adapter.
#[derive(Args, Debug)]
pub struct AuthTarget {
    /// Workspace root or working directory (default: auto-discover from cwd).
    #[arg(long, short = 'p')]
    pub path: Option<PathBuf>,

    /// Print what would be written without touching the filesystem.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Subcommand, Debug)]
pub enum GenerateCommand {
    /// A transport-agnostic port (mod + module + service).
    Feature(GenTarget),
    /// A DB-backed CRUD slice (entity + CrudService + guarded HTTP adapter).
    Resource(GenTarget),
    /// One `#[expose]` entity in an existing feature, without the CRUD slice.
    ///
    /// Placement follows the feature: its first entity is the lone `entity.rs`,
    /// and a feature already keeping several in `entities/` gets one more file
    /// there. The name after the slash is the entity's own — `g entity posts`
    /// writes `Post`, `g entity posts/comment` writes `Comment`.
    Entity(EntityTarget),
    /// The app's authn/authz adapter (Claims, AuthnGuard, AuthzAbility, AuthzGuard).
    Auth(AuthTarget),
    /// A SeaORM migration, registered in both lib.rs and migrator.rs.
    Migration(GenTarget),
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
            output,
            env_prefix,
            check,
            dry_run,
        } => {
            let names = Names::parse(&name);
            let opts = commands::NewOptions {
                name,
                output: output.clone(),
                env_prefix,
                dry_run,
            };
            commands::run_new(opts.clone())?;
            if check && !dry_run {
                run_check(&names, &output)?;
            }
            Ok(())
        }
        Command::Doctor { path } => {
            commands::run_doctor(commands::DoctorOptions { path })?;
            Ok(())
        }
        Command::Lint { path } => {
            commands::run_lint(commands::LintOptions { path })?;
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
        Command::Info { path } => commands::run_info(commands::InfoOptions { path }),
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
            name: t.name,
            path: t.path,
            dry_run: t.dry_run,
        }),
        Entity(t) => commands::run_entity(commands::EntityOptions {
            target: t.target,
            path: t.path,
            dry_run: t.dry_run,
        }),
        Auth(t) => commands::run_auth(commands::AuthOptions {
            path: t.path,
            dry_run: t.dry_run,
        }),
        Migration(t) => commands::run_migration(commands::MigrationOptions {
            name: t.name,
            path: t.path,
            dry_run: t.dry_run,
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

fn run_check(names: &Names, output: &std::path::Path) -> CliResult<()> {
    if let Some(ws) = crate::context::NestrsWorkspace::discover(output)? {
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
        commands::run_cargo_check(&output.join(&names.kebab))?;
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
