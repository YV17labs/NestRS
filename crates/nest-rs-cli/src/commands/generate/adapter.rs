//! `nestrs g http|graphql|ws|queue|schedule|mcp|events <feature>` — bolt one
//! transport adapter onto an existing port. One uniform generator parameterised
//! by [`Transport`]: it picks the right templates, imports what serves the edge
//! into the adapter (an event's bus into the port), ensures the transport's
//! crates, wires the feature `mod.rs`, and (inside an app) the app's imports.
//!
//! A `g resource` port exposes a `CrudService` rather than `g feature`'s
//! `count()` stand-in, and its rows are reachable only behind the app's guards:
//! GraphQL and HTTP over one take the `#[crud]` template, and each transport
//! gets its `authz/` adapter ([`auth::authz_for`]) whenever the workspace has a
//! policy to enforce. An HTTP controller and a WebSocket gateway bind the guards
//! over either port shape, and a GraphQL operation and an MCP tool gate on the
//! port's subject — its entity, or a `#[subject]` written beside the service
//! for a port with none — so no generated operation is open by default: the
//! workspace gets its auth adapter first when it has none.

use std::path::PathBuf;

use super::auth;
use super::cargo::{
    adapter_deps, app_host_deps, auth_deps, ensure_features_deps, ensure_workspace_deps,
    graphql_port_deps,
};
use super::migration::{OUTBOX_MIGRATION, queue_framework_entry};
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
    let (runtime_uses, runtime_imports) = runtime_vars(transport);
    let mut r = Renderer::new(&names)
        .with("handler", names.handler_for(transport))
        .with("handler_mod", transport.handler_mod())
        .with("tmodule", tmodule.clone())
        .with("runtime_uses", runtime_uses)
        .with("runtime_imports", runtime_imports);
    for (key, value) in crate::templates::crud::crud_vars(crud_port, transport) {
        r = r.with(key, value);
    }
    // What an operation's posture names: the port's entity, or the capability
    // a port with none declares, written once by the first adapter gating on it.
    let gated_operation = matches!(transport, Transport::Graphql | Transport::Mcp);
    let port_entity = crud_port || feature_root.join("entity.rs").is_file();
    // Named for its kind, never its type: a port named by its own singular would
    // hold `blog/blog.rs`, a module of its own name.
    let subject_file = feature_root.join("subject.rs");
    let writes_subject = gated_operation && !port_entity && !subject_file.exists();
    r = if port_entity {
        let note = if transport == Transport::Mcp {
            [adapter::GATED_ON_ENTITY, adapter::ROWS_MASKED_AS_JSON].join("\n")
        } else {
            adapter::GATED_ON_ENTITY.to_owned()
        };
        r.with("subject", "{{entity}}Entity")
            .with("subject_use", "Entity as {{entity}}Entity")
            .with("granted", "crate::{{snake}}::Entity")
            .with("posture_note", note)
    } else {
        r.with("subject", "{{entity}}")
            .with("subject_use", "{{entity}}")
            .with("granted", "crate::{{snake}}::{{entity}}")
            .with("posture_note", adapter::GATED_ON_SUBJECT)
    };
    let (handler_tmpl, module_tmpl) = templates_for(transport, crud_port);
    let mut s = Scaffold::new();
    s.create(dir.join(transport.handler_file()), r.render(handler_tmpl));

    // Every transport with an authz adapter scaffolds a guarded handler, which
    // imports that adapter, or the access graph fails the boot on the guards it
    // names.
    let mut module_rs = r.render(module_tmpl);
    let guarded = auth::authz_for(transport);
    if let Some(b) = guarded
        && let Some(wired) = ensure_module_imports(&[(b.feature_path, b.module)])(&module_rs)
    {
        module_rs = wired;
    }
    s.create(dir.join("module.rs"), module_rs);
    s.create(dir.join("mod.rs"), r.render(adapter::MOD));

    // The transport's authz adapter, whenever the workspace has a policy to enforce:
    // without it `/graphql` installs no ability and `/mcp` is deny-all.
    let scaffolded_auth = guarded.is_some() && !auth::exists(&ws);
    let authz = guarded.filter(|b| {
        !b.written_by_g_auth && (scaffolded_auth || auth::exists(&ws)) && !b.exists(&ws)
    });
    let decls = authz.map(auth::AuthzAdapter::decls).unwrap_or_default();
    if let Some(authz) = authz {
        authz.queue(&mut s, &ws);
    }
    let mut bootstrapped = None;
    if scaffolded_auth {
        // This run creates `authz/mod.rs`, so its index lines ride in its contents:
        // there is nothing on disk for an `edit` to resolve against.
        bootstrapped = Some(auth::queue(&mut s, &ws, decls)?);
    } else if authz.is_some() {
        s.edit(ws.features_root().join("authz/mod.rs"), ensure_lines(decls));
    }

    let mut port_lines = Vec::new();
    if writes_subject {
        s.create(subject_file, r.render(crate::templates::feature::SUBJECT));
        port_lines.push("mod subject;".to_string());
        port_lines.push(format!("pub use subject::{};", names.entity()));
    }
    // The payload and its `#[queue]` marker are a producer↔worker contract, so
    // they live at the port; re-exporting the marker keeps the typed
    // `push(Q, job, ..)` reachable from outside the private `queue::processor`.
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
    // folder: `events/` is the edge. Its bus is the port's import, since the
    // service emitting the event injects it.
    if transport == Transport::Events {
        s.create(
            feature_root.join("event.rs"),
            r.render(adapter::EVENTS_EVENT),
        );
        port_lines.push("mod event;".to_string());
        port_lines.push(format!("pub use event::{};", names.event()));
        s.edit(
            feature_root.join("module.rs"),
            ensure_module_imports(&[("nest_rs::events::EventsModule", "EventsModule")]),
        );
    }

    let mut deps = adapter_deps(transport);
    if let Some(authz) = authz {
        deps.extend(authz.deps);
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
    // A queue beside the workspace's database arms the outbox, whose table the
    // framework's migration creates.
    let mut workspace_deps = deps.clone();
    let outbox = transport == Transport::Queue
        && queue_framework_entry(&mut s, &ws, &OUTBOX_MIGRATION, &mut workspace_deps)?;
    if !workspace_deps.is_empty() {
        s.edit(
            ws.root.join("Cargo.toml"),
            ensure_workspace_deps(workspace_deps),
        );
    }
    if !deps.is_empty() {
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
    if let Some(b) = authz {
        imports.push((b.app_path, b.module));
    }
    if scaffolded_auth {
        imports.extend(auth::app_imports());
    }
    let wired_app = wire_into_app(&ctx, &mut s, &imports, None);
    // Read before the edit lands: the app's own binding of a queue backend.
    let backend_bound = transport == Transport::Queue
        && wired_app.as_deref().is_some_and(|module_rs| {
            std::fs::read_to_string(module_rs).is_ok_and(|src| src.contains("RedisQueueModule"))
        });
    // The app crate needs the crate owning the binding its next step names, or
    // that step fails with `E0433`.
    let host_deps = app_host_deps(transport);
    if wired_app.is_some()
        && !host_deps.is_empty()
        && let Some(app) = ctx.current_app.as_ref()
    {
        s.edit(app.join("Cargo.toml"), ensure_features_deps(host_deps));
    }

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("the {} adapter for `{}`", transport.folder(), names.snake),
    )?;
    let grant = gated_operation.then(|| {
        if is_graphql && crud_port {
            Grant::EveryAction(r.render("crate::{{snake}}::Entity"))
        } else {
            Grant::Read(r.render("{{granted}}"))
        }
    });
    print_next_steps(
        &ctx,
        transport,
        &names,
        wired_app.is_some(),
        backend_bound,
        authz,
        grant,
    );
    if outbox {
        println!(
            "  The outbox's migration (`{}`) is registered in crates/migrations: its \
             table keeps a job pushed inside a transaction until the commit. Apply it \
             with `nestrs run db up`.",
            OUTBOX_MIGRATION.name()
        );
    }
    if let Some(secrets) = &bootstrapped {
        auth::print_bootstrapped(secrets);
    }
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
        (Transport::Ws, _) => (adapter::WS_GATEWAY, adapter::MODULE),
        (Transport::Schedule, _) => (adapter::SCHEDULE_TASKS, adapter::MODULE),
        (Transport::Mcp, _) => (adapter::MCP_TOOL, adapter::MODULE),
        (Transport::Events, _) => (adapter::EVENTS_LISTENER, adapter::MODULE),
    }
}

/// [`Transport::runtime`] as the `MODULE` template writes it: its `use` lines,
/// and its entries after the port's in `imports`.
pub(crate) fn runtime_vars(transport: Transport) -> (String, String) {
    let runtime = transport.runtime();
    let uses = runtime
        .iter()
        .map(|path| format!("use {path};\n"))
        .collect();
    let imports = runtime
        .iter()
        .map(|path| format!(", {}", path.rsplit("::").next().unwrap_or(path)))
        .collect();
    (uses, imports)
}

/// What an app binds at its root to run a queue adapter's jobs: the backend
/// is the composition's choice, so no adapter imports it.
const QUEUE_BACKEND: &str =
    "nest_rs::redis::RedisModule::for_root(None) and nest_rs::redis::RedisQueueModule";

/// What a generated GraphQL or MCP adapter waits for in `authz/ability.rs`,
/// its subject named by the path that resolves there.
enum Grant {
    /// The one operation of a single-operation template, under `Read`.
    Read(String),
    /// The `#[crud]` resolver's operations, each under its own action.
    EveryAction(String),
}

fn print_next_steps(
    ctx: &Context,
    transport: Transport,
    names: &Names,
    wired: bool,
    backend_bound: bool,
    authz: Option<&'static auth::AuthzAdapter>,
    grant: Option<Grant>,
) {
    let tmodule = names.module_for(transport);
    println!();
    println!("Next steps:");
    if wired {
        println!("  {tmodule} is wired into the current app, and imports what serves it.");
    } else if ctx.current_app.is_some() {
        println!(
            "  Import `features::{}::{}` in this app: it imports what serves it.",
            names.snake, tmodule,
        );
    } else {
        println!(
            "  Import `features::{}::{}` in the app that serves it: it imports what serves it.",
            names.snake, tmodule,
        );
    }
    if transport == Transport::Queue {
        if !wired {
            println!("  The app importing it binds a queue backend at its root: {QUEUE_BACKEND}.");
        } else if !backend_bound {
            println!(
                "  The app binds no queue backend yet: import {QUEUE_BACKEND} at its root, \
                 where the backend is chosen, so its worker has one to consume."
            );
        }
        println!(
            "  Push from a provider injecting Arc<dyn JobProducer>, with \
             JobProducerExt in scope: `queue.push({}, job, None)`. The app that \
             pushes imports nest_rs::redis::RedisQueueModule beside RedisModule.",
            names.queue()
        );
    }
    match grant {
        Some(Grant::Read(subject)) => println!(
            "  The operation is gated on `{subject}`: it answers FORBIDDEN until \
             AuthzAbility grants it in crates/features/src/authz/ability.rs, \
             `ab.can(nest_rs::authz::Action::Read, {subject});`."
        ),
        Some(Grant::EveryAction(entity)) => println!(
            "  Each operation #[crud] writes is gated on `{entity}` under its own action: \
             each answers FORBIDDEN until AuthzAbility grants that action in \
             crates/features/src/authz/ability.rs, and \
             `ab.can(nest_rs::authz::Action::Manage, {entity});` grants them all."
        ),
        None => {}
    }
    if let Some(authz) = authz {
        println!();
        println!(
            "Also created the {} authz adapter (crates/features/src/authz/{}/):",
            transport.folder(),
            authz.dir,
        );
        for line in authz.rationale {
            println!("  {line}");
        }
    }
}
