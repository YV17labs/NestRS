use std::path::Path;

use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, rustfmt};
use crate::templates::{shared, standalone};

use super::{NewTemplate, queue_env_files};

pub fn scaffold(
    output: &Path,
    names: &Names,
    template: NewTemplate,
    dry_run: bool,
) -> CliResult<()> {
    let root = output.join(&names.kebab);
    if root.exists() {
        return Err(CliError::AlreadyExists(root));
    }

    let r = Renderer::new(names);
    let mut s = Scaffold::new();

    s.create(root.join("Cargo.toml"), r.render(standalone::CARGO));
    s.create(
        root.join("rust-toolchain.toml"),
        r.render(shared::RUST_TOOLCHAIN),
    );
    s.create(root.join(".gitignore"), r.render(shared::GITIGNORE));
    s.create(root.join(".dockerignore"), r.render(shared::DOCKERIGNORE));
    s.create(root.join("Justfile"), r.render(standalone::JUSTFILE));
    s.create(root.join("README.md"), r.render(standalone::README));
    s.create(root.join("Dockerfile"), r.render(standalone::DOCKERFILE));
    queue_env_files(&mut s, &root, names, &names.kebab, shared::ENV);

    queue_sources(&mut s, &root.join("src"), &r, template);

    if matches!(template, NewTemplate::Hello) {
        s.create(root.join("tests/e2e.rs"), r.render(standalone::E2E));
    }

    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }

    println!("Created standalone nestrs app at {}", root.display());
    println!("Template: {}", template.description());
    report.print(output);
    print_next_steps(&root);
    Ok(())
}

fn queue_sources(s: &mut Scaffold, src: &Path, r: &Renderer, template: NewTemplate) {
    let lib = match template {
        NewTemplate::Hello => standalone::LIB_HELLO,
        NewTemplate::Empty => standalone::LIB_EMPTY,
    };
    s.create(src.join("lib.rs"), r.render(lib));
    s.create(src.join("main.rs"), r.render(standalone::MAIN));

    let module_src = match template {
        NewTemplate::Hello => standalone::MODULE_HELLO,
        NewTemplate::Empty => standalone::MODULE_EMPTY,
    };
    s.create(src.join("module.rs"), r.render(module_src));

    if matches!(template, NewTemplate::Hello) {
        s.create(src.join("service.rs"), r.render(standalone::SERVICE));
        s.create(src.join("controller.rs"), r.render(standalone::CONTROLLER));
    }
}

fn print_next_steps(root: &Path) {
    println!();
    println!("Mode: standalone (one crate, logic in src/)");
    println!();
    println!("Next steps:");
    println!("  cd {}", root.display());
    println!("  just dev");
    println!("  Open http://localhost:3000/ in your browser");
    println!();
}
