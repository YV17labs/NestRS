//! `nestrs g http|graphql|ws|queue|schedule|mcp|events <feature>` — bolt one
//! transport adapter onto an existing port. One uniform generator parameterised
//! by [`Transport`]: it picks the right templates, ensures the transport's
//! crates, wires the feature `mod.rs`, and (inside an app) the app's imports.
//!
//! A `g resource` port exposes a `CrudService` rather than `g feature`'s
//! `count()` stand-in, and its rows are reachable only behind the app's guards:
//! GraphQL and HTTP over one take the `#[crud]` template, and each transport
//! gets its `authz/` bridge ([`auth::bridge_for`]) whenever the workspace has a
//! policy to enforce.

use std::path::PathBuf;

use super::auth;
use super::cargo::{
    adapter_deps, app_host_deps, auth_deps, ensure_features_deps, ensure_workspace_deps,
    graphql_port_deps,
};
use super::support::{finish, wire_into_app};
use crate::commands::resolve_start;
use crate::context::{Context, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::{Names, Transport, command_file};
use crate::scaffold::{
    Renderer, Scaffold, ensure_expose_graphql, ensure_lines, ensure_module_imports,
};
use crate::templates::adapter;

pub(crate) struct AdapterOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub(crate) fn run(transport: Transport, opts: AdapterOptions) -> CliResult<()> {
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

    // A guarded `#[crud]` handler imports its transport's authz bridge, or the
    // access graph fails the boot on the guards it names.
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

    // The transport's authz bridge, whenever the workspace has a policy to enforce:
    // without it `/graphql` installs no ability and `/mcp` is deny-all.
    let scaffolded_auth = is_graphql && crud_port && !auth::exists(&ws);
    let bridge = auth::bridge_for(transport).filter(|b| {
        !b.written_by_g_auth && (scaffolded_auth || auth::exists(&ws)) && !b.exists(&ws)
    });
    let decls = bridge.map(auth::AuthzBridge::decls).unwrap_or_default();
    if let Some(bridge) = bridge {
        bridge.queue(&mut s, &ws);
    }
    if scaffolded_auth {
        // This run creates `authz/mod.rs`, so its index lines ride in its contents:
        // there is nothing on disk for an `edit` to resolve against.
        auth::queue(&mut s, &ws, decls);
    } else if bridge.is_some() {
        s.edit(ws.features_root().join("authz/mod.rs"), ensure_lines(decls));
    }

    // The payload and its `#[queue]` marker are a producer↔worker contract, so
    // they live at the port; re-exporting the marker keeps the typed
    // `push(Q, job, ..)` reachable from outside the private `queue::processor`.
    let mut port_lines = Vec::new();
    if transport == Transport::Queue {
        s.create(
            feature_root.join(command_file(&opts.name, 1)),
            r.render(adapter::QUEUE_COMMAND),
        );
        port_lines.push("mod command;".to_string());
        port_lines.push(format!(
            "pub use command::{{{}, {}}};",
            names.command(),
            names.queue()
        ));
    }
    // An event is a published fact, so it too is port vocabulary, with no plural
    // folder: `events/` is the edge.
    if transport == Transport::Events {
        s.create(
            feature_root.join("event.rs"),
            r.render(adapter::EVENTS_EVENT),
        );
        port_lines.push("mod event;".to_string());
        port_lines.push(format!("pub use event::{};", names.event()));
    }

    let mut deps = adapter_deps(transport);
    if let Some(bridge) = bridge {
        deps.extend(bridge.deps);
    }
    // `#[expose(graphql)]` makes the port's entity a GraphQL object; a port
    // without the generated `entity.rs` is left alone, since an `edit` on a missing
    // path fails the whole transaction.
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

    // One edit per file: each `s.edit` re-reads the file from disk.
    port_lines.push(format!("pub mod {};", transport.folder()));
    port_lines.push(format!("pub use {}::{};", transport.folder(), tmodule));
    s.edit(feature_root.join("mod.rs"), ensure_lines(port_lines));

    let use_path = format!("features::{}::{}", names.snake, tmodule);
    let mut imports = vec![(use_path.as_str(), tmodule.as_str())];
    if let Some(b) = bridge {
        imports.push((b.app_path, b.module));
    }
    if scaffolded_auth {
        imports.extend(auth::app_imports());
    }
    let wired_app = wire_into_app(&ctx, &mut s, &imports, None);
    // The app crate needs the crate owning the transport's root module, or the
    // printed next step fails with `E0433`.
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

/// Whether the port's service implements `CrudService`, read from the source:
/// the service's API is what decides which methods an adapter may call.
fn is_crud_port(ws: &NestrsWorkspace, snake: &str) -> bool {
    std::fs::read_to_string(ws.feature_root(snake).join("service.rs"))
        .is_ok_and(|src| src.contains("impl CrudService for"))
}

/// The handler skeleton, and the `module.rs` that serves it.
///
/// A `g resource` port's `CrudService` has no `count()`, so each transport has
/// a CRUD twin: GraphQL and HTTP the guarded `#[crud]` shape, the rest stubs,
/// since their reads need an ambient ability a skeleton must not fabricate.
pub(crate) fn templates_for(transport: Transport, crud_port: bool) -> (&'static str, &'static str) {
    match (transport, crud_port) {
        (Transport::Graphql, true) => (adapter::GRAPHQL_RESOLVER_CRUD, adapter::MODULE),
        (Transport::Http, true) => (crate::templates::resource::HTTP_CONTROLLER, adapter::MODULE),
        // The rest render one template either way; the differing handler arrives as
        // `crud_vars`.
        (Transport::Queue, _) => (adapter::QUEUE_PROCESSOR, adapter::MODULE),
        (Transport::Http, false) => (adapter::HTTP_CONTROLLER, adapter::MODULE),
        (Transport::Graphql, false) => (adapter::GRAPHQL_RESOLVER, adapter::MODULE),
        (Transport::Ws, _) => (adapter::WS_GATEWAY, adapter::WS_MODULE),
        (Transport::Schedule, _) => (adapter::SCHEDULE_TASKS, adapter::MODULE),
        (Transport::Mcp, _) => (adapter::MCP_TOOL, adapter::MODULE),
        (Transport::Events, _) => (adapter::EVENTS_LISTENER, adapter::MODULE),
    }
}

/// The app-level root module each transport needs to actually serve the adapter.
fn host_module(transport: Transport) -> &'static str {
    match transport {
        Transport::Http | Transport::Ws => "nest_rs::http::HttpModule",
        Transport::Graphql => "nest_rs::graphql::GraphqlModule",
        Transport::Queue => {
            "nest_rs::redis::RedisModule::for_root(None) + nest_rs::redis::RedisQueueModule + nest_rs::queue::QueueModule::for_root(None)"
        }
        Transport::Schedule => "nest_rs::schedule::ScheduleModule",
        Transport::Mcp => "nest_rs::http::HttpModule",
        Transport::Events => "nest_rs::events::EventsModule",
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
    if matches!(transport, Transport::Queue) {
        println!(
            "  Push from a provider injecting Arc<dyn JobProducer>, with \
             JobProducerExt in scope: `queue.push({}, job, None)`. The app that \
             pushes imports nest_rs::redis::RedisQueueModule beside RedisModule.",
            names.queue()
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
