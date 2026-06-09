use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ValueEnum;

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::fs::{render, write_file};
use crate::naming::Names;
use crate::templates::app;

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum NewTemplate {
    /// Working `GET /` → "Hello World" (matches `apps/hello`).
    #[default]
    Hello,
    /// HTTP transport only — no routes yet (matches the tutorial scaffold).
    Empty,
}

pub struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    pub template: NewTemplate,
    pub in_workspace: bool,
}

pub fn run(opts: NewOptions) -> CliResult<()> {
    let names = Names::parse(&opts.name);
    if opts.in_workspace {
        let ws = NestrsWorkspace::require(&opts.output)?;
        scaffold_workspace_app(&ws, &names, opts.template)?;
    } else {
        scaffold_standalone(&opts.output, &names, opts.template)?;
    }
    Ok(())
}

fn scaffold_standalone(output: &Path, names: &Names, template: NewTemplate) -> CliResult<()> {
    let root = output.join(&names.kebab);
    if root.exists() {
        return Err(CliError::AlreadyExists(root));
    }

    write_file(&root.join("Cargo.toml"), &render(app::STANDALONE_CARGO, names))?;
    write_file(&root.join(".gitignore"), &render(app::GITIGNORE, names))?;
    write_file(&root.join("README.md"), &render(app::README, names))?;
    write_file(
        &root.join("src/main.rs"),
        &render(app::STANDALONE_MAIN, names),
    )?;
    write_module_files(&root.join("src"), names, template)?;

    println!("Created standalone nestrs app at {}", root.display());
    print_post_create_standalone(&root);
    Ok(())
}

fn scaffold_workspace_app(
    ws: &NestrsWorkspace,
    names: &Names,
    template: NewTemplate,
) -> CliResult<()> {
    let root = ws.apps_root().join(&names.kebab);
    if root.exists() {
        return Err(CliError::AlreadyExists(root));
    }

    write_file(&root.join("Cargo.toml"), &render(app::WORKSPACE_CARGO, names))?;
    write_file(&root.join("src/main.rs"), &render(app::MAIN, names))?;
    write_file(&root.join("src/lib.rs"), &render(app::LIB, names))?;
    write_module_files(&root.join("src"), names, template)?;

    if matches!(template, NewTemplate::Hello) {
        write_file(&root.join("tests/e2e.rs"), &render(app::E2E, names))?;
    }

    println!("Created workspace app at {}", root.display());
    print_post_create_workspace(ws, names);
    Ok(())
}

fn write_module_files(src: &Path, names: &Names, template: NewTemplate) -> CliResult<()> {
    let module_src = match template {
        NewTemplate::Hello => app::MODULE_HELLO,
        NewTemplate::Empty => app::MODULE_EMPTY,
    };
    write_file(&src.join("module.rs"), &render(module_src, names))?;

    if matches!(template, NewTemplate::Hello) {
        write_file(&src.join("service.rs"), &render(app::SERVICE, names))?;
        write_file(&src.join("controller.rs"), &render(app::CONTROLLER, names))?;
    }
    Ok(())
}

fn print_post_create_standalone(root: &Path) {
    println!();
    println!("Next steps:");
    println!("  cd {}", root.display());
    println!("  cargo run");
    println!("  curl http://localhost:3000/");
    println!();
    println!("Optional: run `nestrs doctor` to verify your toolchain.");
}

fn print_post_create_workspace(ws: &NestrsWorkspace, names: &Names) {
    println!();
    println!("Next steps:");
    println!("  cd {}", ws.root.display());
    println!("  just dev {}", names.kebab);
    println!("  cargo check -p {}", names.kebab);
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
