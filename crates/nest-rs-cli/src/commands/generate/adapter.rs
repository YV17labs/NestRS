//! `nestrs g http|graphql|ws|queue|schedule|mcp <feature>` — bolt one
//! transport adapter onto an existing port. One uniform generator parameterised
//! by [`Transport`]: it picks the right templates, ensures the transport's
//! crates, wires the feature `mod.rs`, and (inside an app) the app's imports.
//!
//! Two things vary with the **port**, not the transport: a `g resource` port
//! exposes a `CrudService` rather than the `count()` stand-in `g feature`
//! writes, and a DB-backed adapter is only reachable behind the app's guards.
//! So a GraphQL adapter over a resource takes the `#[crud]` template and comes
//! with the `authz/graphql/` bridge its module imports — the GraphQL twin of
//! what `g resource` already does for HTTP.

use std::path::PathBuf;

use super::auth;
use super::cargo::{
    adapter_deps, auth_deps, ensure_features_deps, ensure_workspace_deps, graphql_authz_deps,
    graphql_port_deps,
};
use super::support::{finish, resolve_start, wire_into_app};
use crate::context::{Context, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::{Names, Transport, command_file};
use crate::scaffold::{
    Renderer, Scaffold, ensure_expose_graphql, ensure_lines, ensure_module_imports,
};
use crate::templates::adapter;

pub struct AdapterOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub fn run(transport: Transport, opts: AdapterOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;

    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);
    if !ws.feature_exists(&names.snake) {
        return Err(CliError::FeatureNotFound {
            name: names.snake.clone(),
        });
    }

    let feature_root = ws.feature_root(&names.snake);
    let dir = feature_root.join(transport.folder());
    if dir.exists() {
        return Err(CliError::AdapterExists {
            transport: transport.folder(),
            name: names.snake.clone(),
            path: dir,
        });
    }

    let handler = names.handler_for(transport);
    let tmodule = names.module_for(transport);
    let r = Renderer::new(&names)
        .with("handler", handler.clone())
        .with("handler_mod", transport.handler_mod())
        .with("tmodule", tmodule.clone());

    // A CRUD port's service has no `count()`, and its rows are only reachable
    // behind an ability — both decide which templates this adapter takes.
    let is_graphql = transport == Transport::Graphql;
    let crud_port = is_crud_port(&ws, &names.snake);
    let (handler_tmpl, module_tmpl) = templates_for(transport, crud_port);
    let mut s = Scaffold::new();
    s.create(dir.join(transport.handler_file()), r.render(handler_tmpl));

    // A guarded resolver is enforced through the GraphQL authz bridge, so its
    // module imports it — through the same wiring transform an app's
    // composition site takes, rather than a second copy of the module template.
    let mut module_rs = r.render(module_tmpl);
    if is_graphql
        && crud_port
        && let Some(wired) = ensure_module_imports(&[(
            "crate::authz::AuthzGraphqlModule",
            "AuthzGraphqlModule",
        )])(&module_rs)
    {
        module_rs = wired;
    }
    s.create(dir.join("module.rs"), module_rs);
    s.create(dir.join("mod.rs"), r.render(adapter::MOD));

    // The GraphQL authz adapter: scaffolded whenever this workspace has a
    // policy to enforce (or is getting one now, because a CRUD resolver names
    // the guards). `/graphql` gates in band, per operation — without these
    // providers the endpoint falls back to a chain that installs no ability,
    // and every `#[authorize]` operation answers on rows nobody scoped.
    let scaffolded_auth = is_graphql && crud_port && !auth::exists(&ws);
    let graphql_authz =
        is_graphql && (crud_port || auth::exists(&ws)) && !auth::graphql_exists(&ws);
    if graphql_authz {
        auth::queue_graphql(&mut s, &ws);
    }
    if scaffolded_auth {
        // The index lines ride in `authz/mod.rs`'s contents: this run creates
        // the file, so there is nothing on disk for an `edit` to resolve
        // against. (Scaffolding auth implies scaffolding its GraphQL bridge —
        // `authz/graphql` cannot exist when `authz` does not.)
        auth::queue(&mut s, &ws, auth::graphql_decls());
    } else if graphql_authz {
        s.edit(
            ws.features_root().join("authz/mod.rs"),
            ensure_lines(auth::graphql_decls()),
        );
    }

    // The queue payload and its `QueueName` marker are a producer↔worker
    // contract, so they live at the *port* (`command.rs`), not in the
    // consumer-side `queue/` adapter — the generated `processor.rs` imports
    // both. Re-exporting the marker is what keeps the typed `push_to::<Q>`
    // reachable: left inside the private `queue::processor` module it is
    // invisible even to the feature's own service, and the untyped
    // `push(name, job)` escape hatch becomes the only way to enqueue. Lines
    // wiring it into the feature `mod.rs` are folded into the single edit
    // below (one edit per file).
    let mut port_lines = Vec::new();
    if transport == Transport::Queue {
        // Single payload today, so `command_file` yields `command.rs`; routing
        // through it keeps the port-object placement rule in one place for when
        // the multi-payload (`commands/<stem>_command.rs`) form gains a caller.
        s.create(
            feature_root.join(command_file(&opts.name, 1)),
            r.render(adapter::QUEUE_COMMAND),
        );
        port_lines.push("mod command;".to_string());
        port_lines.push(format!(
            "pub use command::{{{}, {}}};",
            names.command(),
            names.queue_name()
        ));
    }

    // Ensure the transport's crates — plus the bridge's, and the auth
    // adapter's when this run bootstrapped it. One list, one edit per manifest.
    let mut deps = adapter_deps(transport);
    if graphql_authz {
        deps.extend(graphql_authz_deps());
    }
    // The port's entity has to *be* a GraphQL object before a resolver can
    // return it: `#[expose(graphql)]` is what derives one. A port that keeps
    // its entities elsewhere than the generated `entity.rs` is left alone —
    // an `edit` on a path that does not exist fails the whole transaction.
    if is_graphql && crud_port {
        deps.extend(graphql_port_deps());
        let entity = feature_root.join("entity.rs");
        if entity.is_file() {
            s.edit(entity, ensure_expose_graphql());
        }
    }
    if scaffolded_auth {
        deps.extend(auth_deps());
    }
    if !deps.is_empty() {
        s.edit(
            ws.root.join("Cargo.toml"),
            ensure_workspace_deps(deps.clone()),
        );
        s.edit(ws.features_cargo(), ensure_features_deps(deps));
    }
    if scaffolded_auth {
        s.edit(ws.features_lib(), ensure_lines(auth::lib_decls()));
    }

    // Wire the feature's `mod.rs` to expose the adapter (and, for queue, the
    // port `Command` created above) — one edit, since each `s.edit` re-reads
    // the file from disk.
    port_lines.push(format!("pub mod {};", transport.folder()));
    port_lines.push(format!(
        "pub use {}::{{{}, {}}};",
        transport.folder(),
        handler,
        tmodule
    ));
    s.edit(feature_root.join("mod.rs"), ensure_lines(port_lines));

    // Wire the adapter module into the current app, when the cursor is in one —
    // together with the authz roots this run created, so the composition site
    // stays the inventory of what the app serves.
    let use_path = format!("features::{}::{}", names.snake, tmodule);
    let mut imports = vec![(use_path.as_str(), tmodule.as_str())];
    if graphql_authz {
        imports.push(("features::authz::AuthzGraphqlModule", "AuthzGraphqlModule"));
    }
    if scaffolded_auth {
        imports.extend(auth::APP_IMPORTS);
    }
    let wired_app = wire_into_app(&ctx, &mut s, &imports, None);

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("Added {} adapter to `{}`", transport.folder(), names.snake),
    )?;
    print_next_steps(
        &ctx,
        transport,
        &names,
        &tmodule,
        wired_app.is_some(),
        graphql_authz,
    );
    Ok(())
}

/// Whether the port's service implements `CrudService` — the contract that
/// decides which methods an adapter may call. Read from the source rather than
/// inferred from the folder shape: what breaks the scaffold is the service's
/// API, so that is what is asked.
fn is_crud_port(ws: &NestrsWorkspace, snake: &str) -> bool {
    std::fs::read_to_string(ws.feature_root(snake).join("service.rs"))
        .is_ok_and(|src| src.contains("impl CrudService for"))
}

/// The handler skeleton, and the `module.rs` that serves it. A CRUD port over
/// GraphQL is the one pair that differs: `#[crud]` behind the app's guards,
/// whose module imports the bridge enforcing them.
pub(crate) fn templates_for(transport: Transport, crud_port: bool) -> (&'static str, &'static str) {
    if transport == Transport::Graphql && crud_port {
        return (adapter::GRAPHQL_RESOLVER_CRUD, adapter::MODULE);
    }
    match transport {
        Transport::Http => (adapter::HTTP_CONTROLLER, adapter::MODULE),
        Transport::Graphql => (adapter::GRAPHQL_RESOLVER, adapter::MODULE),
        Transport::Ws => (adapter::WS_GATEWAY, adapter::WS_MODULE),
        Transport::Queue => (adapter::QUEUE_PROCESSOR, adapter::MODULE),
        Transport::Schedule => (adapter::SCHEDULE_TASKS, adapter::MODULE),
        Transport::Mcp => (adapter::MCP_TOOL, adapter::MODULE),
    }
}

/// The app-level root module each transport needs to actually serve the adapter.
fn host_module(transport: Transport) -> &'static str {
    match transport {
        Transport::Http | Transport::Ws => "HttpModule",
        Transport::Graphql => "GraphqlModule",
        Transport::Queue => "QueueWorkerModule",
        Transport::Schedule => "ScheduleModule",
        Transport::Mcp => "HttpModule",
    }
}

fn print_next_steps(
    ctx: &Context,
    transport: Transport,
    names: &Names,
    tmodule: &str,
    wired: bool,
    graphql_authz: bool,
) {
    println!();
    println!("Next steps:");
    if wired {
        println!("  {tmodule} is wired into the current app.");
        println!(
            "  Make sure the app imports {} so the adapter is served.",
            host_module(transport)
        );
    } else if ctx.current_app.is_some() {
        println!(
            "  Import `features::{}::{}` in this app (needs {}).",
            names.snake,
            tmodule,
            host_module(transport)
        );
    } else {
        println!(
            "  Import `features::{}::{}` in an app that has {}.",
            names.snake,
            tmodule,
            host_module(transport)
        );
    }
    if matches!(transport, Transport::Mcp) {
        println!(
            "  Security: the MCP endpoint denies all requests until you bind an \
             McpOperationGuard. Wire `McpAbilityBridge` (features::authz::mcp) so \
             callers are authenticated and the ambient Ability is installed."
        );
    }
    if graphql_authz {
        println!();
        println!("Also created the GraphQL authz adapter (crates/features/src/authz/graphql/):");
        println!("  /graphql has no guard at the HTTP edge — authn and the ability run in band,");
        println!("  per operation, through AuthzGraphqlModule. Every resolver serving rows");
        println!("  imports it and declares #[authorize(Action, Entity)] or #[public].");
    }
}
