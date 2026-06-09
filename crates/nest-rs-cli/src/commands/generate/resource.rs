//! `nestrs g resource <name>` — a DB-backed CRUD slice: an `#[expose]` entity,
//! a `CrudService`, and an HTTP adapter with explicit thin handlers. Missing
//! workspace dependencies are spliced in automatically; the slice compiles as
//! generated. Harden it with `#[crud]` + guards by copying `users/`/`orgs/`.

use std::path::PathBuf;

use super::cargo::{ensure_features_deps, ensure_workspace_deps, resource_deps};
use super::{finish, resolve_start, wire_into_app};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_decl};
use crate::templates::resource;

pub struct ResourceOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub fn run(opts: ResourceOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;

    let names = Names::parse(&opts.name);
    let root = ws.feature_root(&names.snake);
    if root.exists() {
        return Err(CliError::FeatureExists {
            name: names.snake.clone(),
            path: root,
        });
    }

    let r = Renderer::new(&names);
    let mut s = Scaffold::new();

    // DB-backed port.
    s.create(root.join("entity.rs"), r.render(resource::ENTITY));
    s.create(root.join("service.rs"), r.render(resource::SERVICE));
    s.create(root.join("module.rs"), r.render(resource::MODULE));
    s.create(root.join("mod.rs"), r.render(resource::MOD));

    // HTTP adapter.
    s.create(root.join("http/mod.rs"), r.render(resource::HTTP_MOD));
    s.create(root.join("http/module.rs"), r.render(resource::HTTP_MODULE));
    s.create(
        root.join("http/controller.rs"),
        r.render(resource::HTTP_CONTROLLER),
    );

    // Dependencies + feature registration.
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(resource_deps()),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(resource_deps()));
    s.edit(
        ws.features_lib(),
        ensure_decl(&format!("pub mod {};", names.snake)),
    );

    // Wire the HTTP module into the current app — but only when it has a DB,
    // since the resource module needs `DatabaseModule` at boot.
    let wired_app = wire_into_app(
        &ctx,
        &mut s,
        &format!("features::{}::{}", names.snake, names.http_module()),
        &names.http_module(),
        Some("DatabaseModule"),
    );

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("Created resource `{}`", names.snake),
    )?;
    print_next_steps(&ctx, &names, wired_app);
    Ok(())
}

fn print_next_steps(ctx: &Context, names: &Names, wired_app: Option<PathBuf>) {
    println!();
    println!("Next steps:");
    println!("  1. Fill in `entity.rs` columns and add a migration.");
    if wired_app.is_some() {
        println!(
            "  2. {} is wired into the current app.",
            names.http_module()
        );
    } else if ctx.current_app.is_some() {
        println!(
            "  2. Add `DatabaseModule::for_root(None)` to this app, then import \
             `features::{}::{}`.",
            names.snake,
            names.http_module()
        );
    } else {
        println!(
            "  2. Import `features::{}::{}` in an app that has `DatabaseModule`.",
            names.snake,
            names.http_module()
        );
    }
    println!("  3. Add transports:  nestrs g graphql|ws {}", names.kebab);
    println!("  Harden with #[crud] + guards — see crates/features/src/orgs/.");
}
