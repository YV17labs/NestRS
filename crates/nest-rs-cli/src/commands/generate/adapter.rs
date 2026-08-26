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
//! what `g resource` already does for HTTP. GraphQL, WS and MCP each get their
//! own bridge under `authz/` ([`auth::bridge_for`]) whenever the workspace has
//! a policy to enforce.

use std::path::PathBuf;

use super::auth;
use super::cargo::{
    adapter_deps, app_host_deps, auth_deps, ensure_features_deps, ensure_workspace_deps,
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

    let tmodule = names.module_for(transport);
    // A CRUD port's service has no `count()`, and its rows are only reachable
    // behind an ability — both decide the handler this adapter renders, and
    // (for GraphQL and HTTP) which template it takes.
    let is_graphql = transport == Transport::Graphql;
    let crud_port = is_crud_port(&ws, &names.snake);
    let mut r = Renderer::new(&names)
        .with("handler", names.handler_for(transport))
        .with("handler_mod", transport.handler_mod())
        .with("tmodule", tmodule.clone());
    for (key, value) in crate::templates::crud::crud_vars(crud_port, transport) {
        r = r.with(key, value);
    }
    let (handler_tmpl, module_tmpl) = templates_for(transport, crud_port);
    let mut s = Scaffold::new();
    s.create(dir.join(transport.handler_file()), r.render(handler_tmpl));

    // Only the two `#[crud]` templates render a *guarded* handler, and a guarded
    // handler is enforced through its transport's authz bridge — so those two
    // import it in the adapter's `module.rs`, through the same wiring transform
    // an app's composition site takes rather than a second module template.
    // Without it the access graph fails the boot on the guards the generated
    // `#[crud]` block names. The module's name comes from the bridge table, so
    // it is spelled once.
    let mut module_rs = r.render(module_tmpl);
    let guarded_handler = crud_port && matches!(transport, Transport::Graphql | Transport::Http);
    if guarded_handler
        && let Some(b) = auth::bridge_for(transport)
        && let Some(wired) = ensure_module_imports(&[(b.feature_path, b.module)])(&module_rs)
    {
        module_rs = wired;
    }
    s.create(dir.join("module.rs"), module_rs);
    s.create(dir.join("mod.rs"), r.render(adapter::MOD));

    // The transport's authz bridge: scaffolded whenever this workspace has a
    // policy to enforce (or is getting one now, because a CRUD resolver names
    // the guards). `/graphql` and `/mcp` gate in band, per operation — without
    // these providers `/graphql` falls back to a chain that installs no
    // ability, and `/mcp` is deny-all. A WS gateway needs the socket data
    // context that scopes its rows.
    //
    // Generating them is the whole point: the `g mcp` next step names
    // `features::authz::mcp`, and the generated gateway's SECURITY comment
    // names `AuthzWsModule`. Both used to be instructions pointing at modules
    // no generator wrote, leaving the repo's own demo as the only source.
    let scaffolded_auth = is_graphql && crud_port && !auth::exists(&ws);
    let bridge = auth::bridge_for(transport).filter(|b| {
        !b.written_by_g_auth && (scaffolded_auth || auth::exists(&ws)) && !b.exists(&ws)
    });
    let decls = bridge.map(auth::AuthzBridge::decls).unwrap_or_default();
    if let Some(bridge) = bridge {
        bridge.queue(&mut s, &ws);
    }
    if scaffolded_auth {
        // The index lines ride in `authz/mod.rs`'s contents: this run creates
        // the file, so there is nothing on disk for an `edit` to resolve
        // against. (Scaffolding auth implies scaffolding its bridge — an
        // `authz/<transport>` cannot exist when `authz` does not.)
        auth::queue(&mut s, &ws, decls);
    } else if bridge.is_some() {
        s.edit(ws.features_root().join("authz/mod.rs"), ensure_lines(decls));
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
    if let Some(bridge) = bridge {
        deps.extend(bridge.deps);
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
    port_lines.push(format!("pub use {}::{};", transport.folder(), tmodule));
    s.edit(feature_root.join("mod.rs"), ensure_lines(port_lines));

    // Wire the adapter module into the current app, when the cursor is in one —
    // together with the authz roots this run created, so the composition site
    // stays the inventory of what the app serves.
    let use_path = format!("features::{}::{}", names.snake, tmodule);
    let mut imports = vec![(use_path.as_str(), tmodule.as_str())];
    if let Some(b) = bridge {
        imports.push((b.app_path, b.module));
    }
    if scaffolded_auth {
        imports.extend(auth::app_imports());
    }
    let wired_app = wire_into_app(&ctx, &mut s, &imports, None);
    // The app crate needs the crate that owns the transport's root module —
    // the one the printed next step tells the reader to import. Editing the
    // app's `module.rs` while leaving its `Cargo.toml` alone made that step
    // fail with `E0433: failed to resolve: use of undeclared crate`.
    if wired_app.is_some()
        && let Some(app) = ctx.current_app.as_ref()
    {
        s.edit(
            app.join("Cargo.toml"),
            ensure_features_deps(app_host_deps(transport)),
        );
    }

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("the {} adapter for `{}`", transport.folder(), names.snake),
    )?;
    print_next_steps(
        &ctx,
        transport,
        &names,
        &tmodule,
        wired_app.is_some(),
        bridge,
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

/// The handler skeleton, and the `module.rs` that serves it.
///
/// Every transport whose default skeleton calls the `g feature` service's
/// `count()` needs a CRUD twin: a `g resource` port's `CrudService` has no such
/// method, so the shared skeleton produced a workspace that did not compile —
/// against the CLI page's own "a freshly-generated port plus any adapter
/// compiles immediately" guarantee. GraphQL's twin is the full `#[crud]`
/// resolver; the rest are handler-shaped stubs, because their reads need an
/// ambient ability a skeleton must not fabricate.
pub(crate) fn templates_for(transport: Transport, crud_port: bool) -> (&'static str, &'static str) {
    match (transport, crud_port) {
        // A CRUD port's service is a `CrudService`: no `count()`, and its rows
        // are only reachable behind an ability. GraphQL and HTTP get the real
        // guarded shape; the rest are handler-shaped stubs, because their reads
        // need an ambient ability a skeleton must not fabricate.
        (Transport::Graphql, true) => (adapter::GRAPHQL_RESOLVER_CRUD, adapter::MODULE),
        (Transport::Http, true) => (crate::templates::resource::HTTP_CONTROLLER, adapter::MODULE),
        // The rest render one template either way — the handler that differs
        // arrives as `crud_vars`.
        (Transport::Queue, _) => (adapter::QUEUE_PROCESSOR, adapter::MODULE),
        (Transport::Http, false) => (adapter::HTTP_CONTROLLER, adapter::MODULE),
        (Transport::Graphql, false) => (adapter::GRAPHQL_RESOLVER, adapter::MODULE),
        (Transport::Ws, _) => (adapter::WS_GATEWAY, adapter::WS_MODULE),
        (Transport::Schedule, _) => (adapter::SCHEDULE_TASKS, adapter::MODULE),
        (Transport::Mcp, _) => (adapter::MCP_TOOL, adapter::MODULE),
    }
}

/// The app-level root module each transport needs to actually serve the adapter.
fn host_module(transport: Transport) -> &'static str {
    match transport {
        Transport::Http | Transport::Ws => "HttpModule",
        Transport::Graphql => "GraphqlModule",
        Transport::Queue => "RedisModule::for_root(None) + RedisWorkerModule::for_root(None)",
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
    bridge: Option<&'static auth::AuthzBridge>,
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
    if matches!(transport, Transport::Mcp) && bridge.is_none() {
        println!(
            "  Security: the MCP endpoint denies all requests until you bind an \
             McpOperationGuard. Run `nestrs g auth`, then `nestrs g mcp` again — it \
             writes the AuthzMcpModule that authenticates callers and installs the \
             ambient Ability."
        );
    }
    if let Some(bridge) = bridge {
        println!();
        println!(
            "Also created the {} authz adapter (crates/features/src/authz/{}/):",
            transport.folder(),
            bridge.dir,
        );
        for line in bridge.rationale {
            println!("  {line}");
        }
    }
}
