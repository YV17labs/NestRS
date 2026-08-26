use std::path::Path;

use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, rustfmt};
use crate::templates::{hello, shared, standalone};

use super::command::with_env_prefix;
use super::{queue_agent_files, queue_env_files};

pub fn scaffold(output: &Path, names: &Names, env_prefix: &str, dry_run: bool) -> CliResult<()> {
    let root = output.join(&names.kebab);
    if root.exists() {
        return Err(CliError::AlreadyExists(root));
    }

    let r = with_env_prefix(Renderer::new(names), env_prefix);
    let mut s = Scaffold::new();

    s.create(root.join("Cargo.toml"), r.render(standalone::CARGO));
    s.create(
        root.join("rust-toolchain.toml"),
        r.render(shared::RUST_TOOLCHAIN),
    );
    s.create(root.join(".gitignore"), r.render(shared::GITIGNORE));
    s.create(root.join(".dockerignore"), r.render(shared::DOCKERIGNORE));
    s.create(root.join("Justfile"), r.render(standalone::JUSTFILE));
    s.create(root.join("test.just"), r.render(standalone::TEST_JUSTFILE));
    s.create(root.join("README.md"), r.render(standalone::README));
    queue_agent_files(&mut s, &root, &r, shared::AGENTS_STANDALONE_HEAD);
    s.create(root.join("Dockerfile"), r.render(standalone::DOCKERFILE));
    queue_env_files(&mut s, &root, &r, &names.kebab, shared::ENV);

    queue_sources(&mut s, &root.join("src"), names);

    // No live infra involved ⇒ `integration`, never `e2e` (the suite norm).
    // Standalone has no separate feature crate: the crate's root module *is*
    // the module that serves the greeting, so the narrowest boot is that one.
    let smoke = r
        .clone()
        .with("smoke_use", format!("{}::{}", names.snake, names.module()))
        .with("smoke_module", names.module());
    s.create(
        root.join("tests/integration/main.rs"),
        smoke.render(shared::SMOKE),
    );
    // Empty, but present: the test recipes filter on `binary(e2e)`, which
    // nextest refuses to parse when no such binary exists.
    s.create(root.join("tests/e2e/main.rs"), r.render(shared::E2E));

    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }

    println!(
        "{} standalone nestrs app at {}",
        report.verb("Created", "Would create"),
        root.display()
    );
    report.print(output);
    print_next_steps(&root);
    Ok(())
}

fn queue_sources(s: &mut Scaffold, src: &Path, names: &Names) {
    let r = Renderer::new(names);
    s.create(src.join("lib.rs"), r.render(standalone::LIB));
    s.create(src.join("main.rs"), r.render(standalone::MAIN));
    s.create(src.join("module.rs"), r.render(standalone::MODULE));

    // The greeting is the shared `hello` module: same service, same `GET /`,
    // laid out for a single crate instead of a feature folder.
    let hello_r = r.with(
        "service_use",
        format!("crate::service::{}", names.service()),
    );
    s.create(src.join("service.rs"), hello_r.render(hello::SERVICE));
    s.create(src.join("controller.rs"), hello_r.render(hello::CONTROLLER));
}

fn print_next_steps(root: &Path) {
    println!();
    println!("Mode: standalone (one crate, logic in src/)");
    println!();
    println!("Next steps:");
    println!("  cd {}", root.display());
    println!("  nestrs run dev");
    println!("  Open http://localhost:3000/ in your browser");
    println!();
}
