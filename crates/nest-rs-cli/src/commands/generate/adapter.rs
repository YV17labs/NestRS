//! `nestrs g http|graphql|ws|queue|schedule|mcp <feature>` — bolt one
//! transport adapter onto an existing port. One uniform generator parameterised
//! by [`Transport`]: it picks the right templates, ensures the transport's
//! crates, wires the feature `mod.rs`, and (inside an app) the app's imports.

use std::path::PathBuf;

use super::cargo::{adapter_deps, ensure_features_deps, ensure_workspace_deps};
use super::{finish, resolve_start, wire_into_app};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::{Names, Transport};
use crate::scaffold::{Renderer, Scaffold, ensure_lines};
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

    let (handler_tmpl, module_tmpl) = templates_for(transport);
    let mut s = Scaffold::new();
    s.create(dir.join(transport.handler_file()), r.render(handler_tmpl));
    s.create(dir.join("module.rs"), r.render(module_tmpl));
    s.create(dir.join("mod.rs"), r.render(adapter::MOD));

    // The queue payload is a producer↔worker contract, so it lives at the
    // *port* (`command.rs`), not in the consumer-side `queue/` adapter — the
    // generated `processor.rs` imports it. Lines wiring it into the feature
    // `mod.rs` are folded into the single edit below (one edit per file).
    let mut port_lines = Vec::new();
    if transport == Transport::Queue {
        s.create(
            feature_root.join("command.rs"),
            r.render(adapter::QUEUE_COMMAND),
        );
        port_lines.push("mod command;".to_string());
        port_lines.push(format!("pub use command::{};", names.command()));
    }

    // Ensure the transport's crates.
    let deps = adapter_deps(transport);
    if !deps.is_empty() {
        s.edit(
            ws.root.join("Cargo.toml"),
            ensure_workspace_deps(deps.clone()),
        );
        s.edit(ws.features_cargo(), ensure_features_deps(deps));
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

    // Wire the adapter module into the current app, when the cursor is in one.
    let wired_app = wire_into_app(
        &ctx,
        &mut s,
        &format!("features::{}::{}", names.snake, tmodule),
        &tmodule,
        None,
    );

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("Added {} adapter to `{}`", transport.folder(), names.snake),
    )?;
    print_next_steps(&ctx, transport, &names, &tmodule, wired_app.is_some());
    Ok(())
}

fn templates_for(transport: Transport) -> (&'static str, &'static str) {
    match transport {
        Transport::Http => (adapter::HTTP_CONTROLLER, adapter::MODULE),
        Transport::Graphql => (adapter::GRAPHQL_RESOLVER, adapter::MODULE),
        Transport::Ws => (adapter::WS_GATEWAY, adapter::MODULE),
        Transport::Queue => (adapter::QUEUE_PROCESSOR, adapter::QUEUE_MODULE),
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
}
