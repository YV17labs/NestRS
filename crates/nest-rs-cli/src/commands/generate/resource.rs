//! `nestrs g resource <name>` — a DB-backed CRUD slice: an `#[expose]` entity,
//! a `CrudService`, and a `#[crud]` HTTP controller behind the app's guards.
//!
//! **Guards are not optional here, by construction.** Every read `Repo` runs is
//! filtered by the caller's ambient `Ability`, and an HTTP request has no
//! ability unless an `AbilityGuard` installed one — so an unguarded DB-backed
//! controller compiles, mounts, and then serves nothing. The generator
//! therefore emits the guarded shape and bootstraps the `g auth` adapter when
//! the workspace has none.

use std::path::PathBuf;

use super::auth;
use super::cargo::{auth_deps, ensure_features_deps, ensure_workspace_deps, resource_deps};
use super::support::{finish, resolve_start, wire_into_app};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_lines};
use crate::templates::resource;

pub struct ResourceOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub fn run(opts: ResourceOptions) -> CliResult<()> {
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
    let scaffolded_auth = !auth::exists(&ws);
    if scaffolded_auth {
        auth::queue(&mut s, &ws, Vec::new());
    }

    // DB-backed port.
    s.create(root.join("entity.rs"), r.render(resource::ENTITY));
    s.create(root.join("service.rs"), r.render(resource::SERVICE));
    s.create(root.join("module.rs"), r.render(resource::MODULE));
    s.create(root.join("mod.rs"), r.render(resource::MOD));

    // HTTP adapter — `#[crud]` behind AuthnGuard + AuthzGuard.
    s.create(root.join("http/mod.rs"), r.render(resource::HTTP_MOD));
    s.create(root.join("http/module.rs"), r.render(resource::HTTP_MODULE));
    s.create(
        root.join("http/controller.rs"),
        r.render(resource::HTTP_CONTROLLER),
    );

    // Dependencies + feature registration. Exactly one `edit` per path,
    // folding in the auth adapter's when this run bootstrapped it: a second
    // `edit` on the same file re-reads it from disk and clobbers the first.
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

    // Wire the HTTP module into the current app — but only when it has a DB,
    // since the resource module needs `DatabaseModule` at boot. The auth roots
    // ride along in the same edit when this run scaffolded them, so the
    // composition site stays the inventory `g auth` would have written.
    let use_path = format!("features::{}::{}", names.snake, names.http_module());
    let http_module = names.http_module();
    let mut imports = vec![(use_path.as_str(), http_module.as_str())];
    if scaffolded_auth {
        imports.extend(auth::APP_IMPORTS);
    }
    let wired_app = wire_into_app(&ctx, &mut s, &imports, Some("DatabaseModule"));

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("resource `{}`", names.snake),
    )?;
    print_next_steps(&ctx, &names, wired_app, scaffolded_auth);
    Ok(())
}

fn print_next_steps(
    ctx: &Context,
    names: &Names,
    wired_app: Option<PathBuf>,
    scaffolded_auth: bool,
) {
    let snake = &names.snake;
    println!();
    println!("Next steps:");
    println!("  1. Fill in `entity.rs` columns, then:  nestrs g migration create_{snake}");
    println!("  2. Grant the ability in `crates/features/src/authz/ability.rs` — until you");
    println!("     do, every route answers 403 and no row crosses the data layer:");
    println!();
    println!("       use crate::{snake} as {snake}_entity;");
    println!("       ab.can(Action::Manage, {snake}_entity::Entity);");
    println!();
    if wired_app.is_some() {
        println!(
            "  3. {} is wired into the current app.",
            names.http_module()
        );
    } else if ctx.current_app.is_some() {
        println!(
            "  3. Add `DatabaseModule::for_root(None)` to this app, then import \
             `features::{}::{}`.",
            snake,
            names.http_module()
        );
    } else {
        println!(
            "  3. Import `features::{}::{}` in an app that has `DatabaseModule`.",
            snake,
            names.http_module()
        );
    }
    println!("  4. Add transports:  nestrs g graphql|ws {}", names.kebab);
    if scaffolded_auth {
        println!();
        println!("Also created the auth adapter (identity/, authn/, authz/) the guards need,");
        println!("plus a development HS256 secret in `.env` — replace it before deploying.");
        println!();
        // The adapter includes a route that mints bearer tokens with no
        // credential — announced here because `g resource` writes it as a side
        // effect, so a reader who never ran `g auth` would meet it only in a
        // boot warning whose remedy is to import it.
        println!("It includes `POST /auth/dev-token`, which mints a token with no credential so");
        println!("your guarded routes are callable at once. It refuses to boot outside");
        println!("development and test. Import `features::authn::AuthnHttpModule` to serve it,");
        println!("or delete `crates/features/src/authn/http/` and write the real login route.");
    }
}
