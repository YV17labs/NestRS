//! `nestrs g http|graphql|ws|queue|schedule|mcp|events <feature>` — bolt one
//! transport adapter onto an existing port. One uniform generator parameterised
//! by [`Transport`]: it picks the right templates, imports an event's bus into
//! the port, ensures the transport's crates, wires the feature `mod.rs`, and
//! (inside an app) the app's imports.
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

use std::path::{Path, PathBuf};

use super::auth;
use super::cargo::{
    adapter_deps, app_host_deps, auth_deps, ensure_features_deps, ensure_workspace_deps,
    graphql_port_deps,
};
use super::migration::{OUTBOX_MIGRATION, queue_framework_entry};
use super::port::{CrudPort, Port};
use super::support::{Import, Wiring, data_layer, finish, wire_into_app, wiring_lines};
use crate::commands::resolve_start;
use crate::context::Context;
use crate::error::{CliError, CliResult, read_if_exists};
use crate::naming::{Names, Transport, command_file};
use crate::scaffold::{
    Renderer, Scaffold, Transform, declares_expose, ensure_expose_graphql, ensure_lines,
    ensure_module_imports, rel,
};
use crate::templates::adapter;

pub(crate) struct AdapterOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    /// The app to wire into (`--app`).
    pub app: Option<String>,
    pub dry_run: bool,
}

pub(crate) fn run(transport: Transport, opts: AdapterOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;
    let target = ctx.target_app(opts.app.as_deref())?;

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
    let port = Port::read(&feature_root, &names)?;
    let crud_port = port.crud.is_some();
    // What an operation's posture names: the port's entity, or the capability
    // a port with none declares, written once by the first adapter gating on it.
    let gated_operation = matches!(transport, Transport::Graphql | Transport::Mcp);
    // Named for its kind, never its type: a port named by its own singular would
    // hold `blog/blog.rs`, a module of its own name.
    let subject_file = feature_root.join("subject.rs");
    let writes_subject = gated_operation && !port.has_entity && !subject_file.exists();
    let mut r = adapter_renderer(&names, transport, crud_port, port.has_entity);
    if let Some(crud) = &port.crud {
        r = r.with("service", crud.service.clone());
    }
    let (handler_tmpl, module_tmpl) = templates_for(transport, crud_port);
    // A `#[crud]` handler runs every call in a unit of work, so the app gets the
    // data layer beside it, as `g resource` writes it: an app without is refused at
    // boot.
    let needs_unit_of_work = crud_port && matches!(transport, Transport::Http | Transport::Graphql);
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
    let mut unflagged = None;
    if is_graphql && let Some(crud) = &port.crud {
        deps.extend(graphql_port_deps());
        unflagged = flag_entity(&mut s, crud, &ws.root)?;
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

    // One edit per file: each `s.edit` re-reads the file from disk. A handler
    // that logs names its feature's target, declared here when the port has none.
    port_lines.push(format!("pub mod {};", transport.folder()));
    port_lines.push(format!("pub use {}::{};", transport.folder(), tmodule));
    let log_target = transport
        .logs()
        .then(|| r.render(crate::templates::feature::TARGET));
    s.edit(
        feature_root.join("mod.rs"),
        ensure_target_and_lines(log_target, port_lines),
    );

    // The edge, and what it runs on that the app's root chooses: the data layer
    // under a `#[crud]` handler, a queue's backend, the token route of the auth
    // adapter this run wrote.
    let edge = Import::new(format!("features::{}::{}", names.snake, tmodule));
    let mut beside = Vec::new();
    if needs_unit_of_work {
        beside.push(data_layer());
    }
    if transport == Transport::Queue {
        beside.push(queue_backend());
    }
    if scaffolded_auth {
        beside.push(auth::app_import());
    }
    let wiring = wire_into_app(&target, &mut s, &edge, &beside)?;
    // The app crate needs the crate owning the binding it now names, or its
    // build fails with `E0433`.
    let host_deps = app_host_deps(transport);
    if let Wiring::Wired { root, .. } = &wiring
        && !host_deps.is_empty()
    {
        s.edit(root.join("Cargo.toml"), ensure_features_deps(host_deps));
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
        transport,
        &names,
        Placed {
            wiring: &wiring,
            edge: &edge,
            beside: &beside,
        },
        authz,
        grant,
        unflagged.as_deref(),
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

/// Queue `#[expose(graphql)]`, which makes the entity `crud` names a GraphQL object;
/// when it cannot take it, what the next step says of why: an `edit` on a missing
/// path would fail the whole transaction, and one finding no attribute writes nothing.
fn flag_entity(s: &mut Scaffold, crud: &CrudPort, base: &Path) -> CliResult<Option<String>> {
    let source = match &crud.entity {
        Some(entity) => read_if_exists(entity)?,
        None => None,
    };
    let (Some(entity), Some(source)) = (&crud.entity, source) else {
        return Ok(Some(format!(
            "{} names an entity this run could not follow to its file",
            rel(&crud.file, base)
        )));
    };
    if !declares_expose(&source) {
        return Ok(Some(format!(
            "{} holds no `#[expose(...)]` for this run to flag",
            rel(entity, base)
        )));
    }
    s.edit(entity, ensure_expose_graphql());
    Ok(None)
}

/// What an adapter for `transport` over `names`' port is written with: the
/// port's names, the transport's, the handler a `CrudService` port takes, and
/// what its operation is gated on — the port's entity, or, over a port with
/// none, the `#[subject]` written beside its service.
pub(crate) fn adapter_renderer(
    names: &Names,
    transport: Transport,
    crud_port: bool,
    port_entity: bool,
) -> Renderer {
    let mut r = Renderer::new(names)
        .with("handler", names.handler_for(transport))
        .with("handler_mod", transport.handler_mod())
        .with("tmodule", names.module_for(transport));
    for (key, value) in crate::templates::crud::crud_vars(crud_port, transport) {
        r = r.with(key, value);
    }
    if port_entity {
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
    }
}

/// `target` above everything in a port's `mod.rs` that declares none, then
/// `lines` in place — one transform, since the file takes one edit.
fn ensure_target_and_lines(target: Option<String>, lines: Vec<String>) -> Transform {
    let lines = ensure_lines(lines);
    Box::new(move |current: &str| {
        let declared = current
            .lines()
            .any(|line| line.trim_start().starts_with("pub const TARGET:"));
        let targeted = target
            .as_ref()
            .filter(|_| !declared)
            .map(|target| format!("{target}\n{current}"));
        let base = targeted.as_deref().unwrap_or(current);
        lines(base).or(targeted)
    })
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

/// What an app binds at its root to run a queue adapter's jobs: the backend
/// is the composition's choice, so no adapter imports it, and the generator
/// writes it into the root that binds none. It imports the connection it runs on.
fn queue_backend() -> Import {
    Import::new("nest_rs::redis::RedisQueueModule")
}

/// What a generated GraphQL or MCP adapter waits for in `authz/ability.rs`,
/// its subject named by the path that resolves there.
enum Grant {
    /// The one operation of a single-operation template, under `Read`.
    Read(String),
    /// The `#[crud]` resolver's operations, each under its own action.
    EveryAction(String),
}

/// Where the adapter went: the wiring, the edge, and what was written or is to
/// be written beside it.
struct Placed<'a> {
    wiring: &'a Wiring,
    edge: &'a Import,
    beside: &'a [Import],
}

fn print_next_steps(
    transport: Transport,
    names: &Names,
    placed: Placed<'_>,
    authz: Option<&'static auth::AuthzAdapter>,
    grant: Option<Grant>,
    unflagged: Option<&str>,
) {
    println!();
    println!("Next steps:");
    for line in wiring_lines(placed.wiring, placed.edge, placed.beside) {
        println!("  {line}");
    }
    if transport == Transport::Queue {
        println!(
            "  Push from a provider injecting Arc<nest_rs::queue::Jobs>: \
             `jobs.push({}, job)`. The app that pushes imports \
             nest_rs::redis::RedisQueueModule.",
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
    if let Some(why) = unflagged {
        println!(
            "  {why}: add `#[expose(graphql)]` to the entity, which the `#[crud]` resolver \
             needs."
        );
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
