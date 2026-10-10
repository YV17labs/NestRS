//! `nestrs g resource <name>` — a DB-backed CRUD slice: an `#[expose]` entity,
//! a `CrudService`, and a `#[crud]` HTTP controller behind the app's guards.
//!
//! **Guards are not optional**: `Repo` filters every read by the ambient
//! `Ability` only an `AbilityGuard` installs, so the generator emits the guarded
//! shape and bootstraps `g auth` when the workspace has none.

use std::path::PathBuf;

use super::auth;
use super::cargo::{auth_deps, ensure_features_deps, ensure_workspace_deps, resource_deps};
use super::support::{finish, wire_into_app};
use crate::commands::resolve_start;
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_lines};
use crate::templates::resource;

pub(crate) struct ResourceOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub(crate) fn run(opts: ResourceOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;

    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
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

    // The guards the controller binds have to exist before it names them.
    let bootstrapped = if auth::exists(&ws) {
        None
    } else {
        Some(auth::queue(&mut s, &ws, Vec::new())?)
    };
    let scaffolded_auth = bootstrapped.is_some();

    s.create(root.join("entity.rs"), r.render(resource::ENTITY));
    s.create(root.join("service.rs"), r.render(resource::SERVICE));
    s.create(root.join("module.rs"), r.render(resource::MODULE));
    s.create(root.join("mod.rs"), r.render(resource::MOD));

    s.create(root.join("http/mod.rs"), r.render(resource::HTTP_MOD));
    s.create(root.join("http/module.rs"), r.render(resource::HTTP_MODULE));
    s.create(
        root.join("http/controller.rs"),
        r.render(resource::HTTP_CONTROLLER),
    );

    // Exactly one `edit` per path: a second re-reads the file from disk and
    // clobbers the first.
    let mut deps = resource_deps();
    let mut decls = Vec::new();
    if scaffolded_auth {
        deps.extend(auth_deps());
        decls.extend(auth::lib_decls());
    }
    decls.push(format!("pub mod {};", names.snake));
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(deps.clone()),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(deps));
    s.edit(ws.features_lib(), ensure_lines(decls));

    // Only into an app with a DB: the resource module needs
    // `SeaOrmDatabaseModule` at boot.
    let use_path = format!("features::{}::{}", names.snake, names.http_module());
    let http_module = names.http_module();
    let mut imports = vec![(use_path.as_str(), http_module.as_str())];
    if scaffolded_auth {
        imports.extend(auth::app_imports());
    }
    let wired_app = wire_into_app(&ctx, &mut s, &imports, Some("SeaOrmDatabaseModule"));

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("resource `{}`", names.snake),
    )?;
    print_next_steps(&ctx, &names, wired_app, bootstrapped.as_ref());
    Ok(())
}

fn print_next_steps(
    ctx: &Context,
    names: &Names,
    wired_app: Option<PathBuf>,
    bootstrapped: Option<&auth::Secrets>,
) {
    let snake = &names.snake;
    println!();
    println!("Next steps:");
    println!("  1. Fill in `entity.rs` columns, then:  nestrs g migration create_{snake}");
    println!("  2. Grant the ability in `define`, in `crates/features/src/authz/ability.rs`, its");
    println!("     `_ab` parameter becoming `ab` — until you do, every route answers 403 and no");
    println!("     row crosses the data layer:");
    println!();
    println!("       ab.can(nest_rs::authz::Action::Manage, crate::{snake}::Entity);");
    println!();
    if wired_app.is_some() {
        println!(
            "  3. {} is wired into the current app.",
            names.http_module()
        );
    } else if ctx.current_app.is_some() {
        println!(
            "  3. Add `SeaOrmModule::for_root(None)` and `SeaOrmDatabaseModule` to this app, then import \
             `features::{}::{}`.",
            snake,
            names.http_module()
        );
    } else {
        println!(
            "  3. Import `features::{}::{}` in an app that has `SeaOrmDatabaseModule`.",
            snake,
            names.http_module()
        );
    }
    println!("  4. Add transports:  nestrs g graphql|ws {}", names.kebab);
    if let Some(secrets) = bootstrapped {
        auth::print_bootstrapped(secrets);
    }
}
