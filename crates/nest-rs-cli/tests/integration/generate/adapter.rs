//! `nestrs g http|graphql|ws|queue|schedule|mcp` — one uniform generator, so the
//! obligations are per transport: the right template, the crates its expansion
//! names, and the wiring into feature and app.

use crate::harness::{
    assigned, run_ok, scaffolded_var, write_fake_app, write_fake_migrations_crate,
    write_fake_workspace,
};
use std::fs;
use std::process::Command;

#[test]
fn generate_http_adapter_wires_feature_mod() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "http", "posts", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("http/controller.rs").is_file());
    assert!(feature.join("http/module.rs").is_file());

    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("pub mod http;"));
    assert!(mod_rs.contains("PostsHttpModule"));
    // The index exports the module and never the handler.
    assert!(!mod_rs.contains("PostsController"), "{mod_rs}");
}

/// Every route declares a posture.
#[test]
fn generate_http_adapter_declares_a_route_posture() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "http", "posts", "-p", path]);

    let controller = fs::read_to_string(
        dir.path()
            .join("crates/features/src/posts/http/controller.rs"),
    )
    .unwrap();
    assert!(
        controller.contains("#[public]"),
        "the scaffolded route must declare its posture: {controller}"
    );
    assert!(
        controller.contains("SECURITY:"),
        "and say why it is open, the way the graphql adapter does: {controller}"
    );
}

#[test]
fn generate_ws_adapter_ensures_dep_and_wires() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    assert!(
        dir.path()
            .join("crates/features/src/posts/ws/gateway.rs")
            .is_file()
    );
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("\"ws\""), "{features_cargo}");
}

/// A self-mount path is its exclusive namespace, so two gateways cannot share
/// one.
#[test]
fn generate_ws_adapter_gives_each_gateway_a_distinct_path() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "feature", "notify", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "notify", "-p", path]);

    let read = |feature: &str| {
        fs::read_to_string(
            dir.path()
                .join(format!("crates/features/src/{feature}/ws/gateway.rs")),
        )
        .unwrap()
    };
    let posts = read("posts");
    let notify = read("notify");
    assert!(
        posts.contains(r#"path = "/ws/posts""#),
        "the gateway path carries the feature name: {posts}"
    );
    assert!(
        notify.contains(r#"path = "/ws/notify""#),
        "so a second adapter does not collide: {notify}"
    );

    // A controller prefix and a self-mount on one path collide too.
    run_ok(dir.path(), &["g", "http", "posts", "-p", path]);
    let controller = fs::read_to_string(
        dir.path()
            .join("crates/features/src/posts/http/controller.rs"),
    )
    .unwrap();
    assert!(
        controller.contains(r#"path = "/posts""#) && !posts.contains(r#"path = "/posts""#),
        "the two adapters of one feature must claim different paths",
    );
}

/// `WsModule` provides the connection registry every default-namespace gateway
/// reads; without it the app dies at boot.
#[test]
fn generate_ws_adapter_imports_the_connection_registry() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    let module_rs =
        fs::read_to_string(dir.path().join("crates/features/src/posts/ws/module.rs")).unwrap();
    assert!(
        module_rs.contains("use nest_rs::ws::WsModule;")
            && module_rs.contains("imports = [PostsModule, WsModule]"),
        "the generated ws module must import WsModule: {module_rs}"
    );
}

/// `#[messages]` expands to names behind `nest_rs_guards`' `ws` feature, and the
/// gateway body logs. Feature unification would hide a miss from
/// `cargo check --workspace`.
#[test]
fn generate_ws_adapter_enables_the_guards_ws_feature_and_tracing() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    let features_cargo_path = dir.path().join("crates/features/Cargo.toml");
    fs::write(
        &features_cargo_path,
        "[package]\nname = \"features\"\n\n[dependencies]\n\
         nest-rs.workspace = true\n",
    )
    .unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    let features_cargo = fs::read_to_string(&features_cargo_path).unwrap();
    assert!(
        features_cargo
            .lines()
            .any(|l| l.starts_with("nest-rs") && l.contains("\"ws\"")),
        "the ws capability must be a feature of nest-rs: {features_cargo}"
    );
    assert!(features_cargo.contains("tracing"), "{features_cargo}");
}

/// A typed WS payload reaches its derives through `#[input]`, so the manifest
/// gains a feature and not a `serde` entry.
#[test]
fn generate_ws_adapter_leaves_serde_to_the_decorator() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    let features_cargo_path = dir.path().join("crates/features/Cargo.toml");
    fs::write(
        &features_cargo_path,
        "[package]
name = \"features\"

[dependencies]
nest-rs.workspace = true
",
    )
    .unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    let features_cargo = fs::read_to_string(&features_cargo_path).unwrap();
    assert!(!features_cargo.contains("serde"), "{features_cargo}");
    assert!(features_cargo.contains("\"ws\""), "{features_cargo}");
}

/// A `g resource` port's `CrudService` has no `count()`.
#[test]
fn generate_ws_over_a_resource_port_does_not_call_count() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", root]);

    let gateway =
        fs::read_to_string(dir.path().join("crates/features/src/posts/ws/gateway.rs")).unwrap();
    assert!(
        !gateway.contains("svc.count()"),
        "a CrudService has no count(): {gateway}",
    );
}

/// The schedule skeleton files no line of its own: `nest-rs-schedule` already
/// emits one `info` per tick.
#[test]
fn generate_schedule_adapter_does_not_restate_the_tick_line() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "schedule", "posts", "-p", path]);

    let tasks_rs = fs::read_to_string(
        dir.path()
            .join("crates/features/src/posts/schedule/tasks.rs"),
    )
    .unwrap();
    assert!(
        !tasks_rs.contains("tracing::"),
        "the tick's own line is the scheduler's to file: {tasks_rs}",
    );
    // …and brings no `tracing` dependency.
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        !features_cargo.contains("tracing"),
        "the schedule skeleton writes no `tracing::` call, so it brings no \
         `tracing` dependency: {features_cargo}"
    );
}

/// `events/` is the edge folder, so the fact the listener receives sits at the
/// port as `event.rs` — never in a plural `events/` beside the edge.
#[test]
fn generate_events_adapter_puts_the_event_at_the_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "events", "posts", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");
    let event_rs = fs::read_to_string(feature.join("event.rs")).unwrap();
    assert!(
        event_rs.contains("pub struct PostChangedEvent"),
        "{event_rs}"
    );

    let listener_rs = fs::read_to_string(feature.join("events/listener.rs")).unwrap();
    assert!(listener_rs.contains("#[listeners]"), "{listener_rs}");
    assert!(
        listener_rs.contains("event: PostChangedEvent) -> Result<()>"),
        "a listener answers a Result its unit settles on: {listener_rs}"
    );
    assert!(!listener_rs.contains("pub struct PostChangedEvent"));

    let module_rs = fs::read_to_string(feature.join("events/module.rs")).unwrap();
    assert!(
        module_rs.contains("pub struct PostsEventsModule"),
        "{module_rs}"
    );

    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("mod event;"));
    assert!(mod_rs.contains("pub use event::PostChangedEvent;"));
    assert!(mod_rs.contains("pub mod events;"));
}

#[test]
fn generate_queue_adapter_puts_command_at_the_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "queue", "posts", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");

    let command_rs = fs::read_to_string(feature.join("command.rs")).unwrap();
    assert!(command_rs.contains("pub struct ProcessPostCommand"));

    let processor_rs = fs::read_to_string(feature.join("queue/processor.rs")).unwrap();
    // rustfmt may reorder the braced list, so assert on the names, not the line.
    let port_import = processor_rs
        .lines()
        .find(|l| l.starts_with("use crate::posts::"))
        .expect("the processor imports its port");
    assert!(
        port_import.contains("ProcessPostCommand") && port_import.contains("PostsQueue"),
        "{port_import}"
    );
    assert!(processor_rs.contains("job: ProcessPostCommand"));
    assert!(!processor_rs.contains("pub struct ProcessPostCommand"));

    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("mod command;"));
    let command_export = mod_rs
        .lines()
        .find(|l| l.starts_with("pub use command::"))
        .expect("the port re-exports its command");
    assert!(
        command_export.contains("ProcessPostCommand") && command_export.contains("PostsQueue"),
        "{command_export}"
    );
    assert!(mod_rs.contains("pub mod queue;"));
    assert!(mod_rs.contains("PostsQueueModule"));
}

/// The `#[queue]` marker is the destination a typed `push` takes, so it is
/// reachable from the feature's own service.
#[test]
fn generate_queue_adapter_declares_the_queue_marker_at_the_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "queue", "posts", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");
    let command_rs = fs::read_to_string(feature.join("command.rs")).unwrap();
    assert!(
        command_rs.contains("#[queue(name = \"posts\", job = ProcessPostCommand)]")
            && command_rs.contains("pub struct PostsQueue;"),
        "the marker belongs beside the payload it names: {command_rs}"
    );

    let processor_rs = fs::read_to_string(feature.join("queue/processor.rs")).unwrap();
    assert!(
        !processor_rs.contains("pub struct PostsQueue"),
        "and nowhere else: {processor_rs}"
    );
    assert!(processor_rs.contains("#[process(queue = PostsQueue"));

    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("PostsQueue"), "{mod_rs}");
}

/// The generated processor module imports the port like every sibling adapter;
/// a processor delegating to the port service needs it at boot.
#[test]
fn generate_queue_adapter_module_imports_the_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "queue", "posts", "-p", path]);

    let module_rs =
        fs::read_to_string(dir.path().join("crates/features/src/posts/queue/module.rs")).unwrap();
    assert!(
        module_rs.contains("imports = [PostsModule]"),
        "the queue adapter module must import its port: {module_rs}"
    );

    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(!features_cargo.contains("tracing"), "{features_cargo}");
}

/// The generator wires `RedisModule` + `RedisQueueModule` and the feature
/// that compiles the queue binding, `redis-queue` — `redis` alone is the
/// connection, as `/queue/` documents.
#[test]
fn generate_queue_adapter_brings_the_queue_binding_feature() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "audio", "-p", root]);
    run_ok(dir.path(), &["g", "queue", "audio", "-p", root]);

    let root_cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(root_cargo.contains("\"redis-queue\""), "{root_cargo}");
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        features_cargo.contains("\"redis-queue\""),
        "{features_cargo}"
    );
}

/// A queue beside the workspace's database arms the outbox, whose table the
/// framework's migration creates: `g queue` registers it in the migrator the
/// workspace keeps, and leaves a workspace without one alone.
#[test]
fn generate_queue_adapter_registers_the_outbox_migration() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "audio", "-p", root]);
    let printed = run_ok(dir.path(), &["g", "queue", "audio", "-p", root]);

    let migrator =
        fs::read_to_string(dir.path().join("crates/migrations/src/migrator.rs")).unwrap();
    assert_eq!(
        migrator
            .matches("Box::new(nest_rs::seaorm::queue::OutboxMigration)")
            .count(),
        1,
        "{migrator}"
    );
    assert!(
        migrator.contains("Box::new(m20260101_000000_init::Migration)"),
        "{migrator}"
    );
    assert!(printed.contains("nestrs run db up"), "{printed}");

    let bare = tempfile::tempdir().unwrap();
    write_fake_workspace(bare.path());
    let root = bare.path().to_str().unwrap();
    run_ok(bare.path(), &["g", "feature", "audio", "-p", root]);
    run_ok(bare.path(), &["g", "queue", "audio", "-p", root]);
    assert!(!bare.path().join("crates/migrations").exists());
}

/// `#[mcp]` expands to names behind the `mcp` feature, so the generator turns
/// the feature on.
#[test]
fn generate_mcp_adapter_enables_the_mcp_feature() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "tools", "-p", root]);
    run_ok(dir.path(), &["g", "mcp", "tools", "-p", root]);

    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        features_cargo.contains("nest-rs") && features_cargo.contains("\"mcp\""),
        "`#[mcp]` needs the `mcp` feature: {features_cargo}",
    );
}

/// `#[resolver]` expands to names behind the `graphql` feature, so the generator
/// turns the *feature* on rather than adding the entry.
#[test]
fn generate_graphql_adapter_enables_the_guards_graphql_feature() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    let features_cargo_path = dir.path().join("crates/features/Cargo.toml");
    fs::write(
        &features_cargo_path,
        "[package]\nname = \"features\"\n\n[dependencies]\nnest-rs.workspace = true\n",
    )
    .unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "graphql", "posts", "-p", path]);

    let features_cargo = fs::read_to_string(&features_cargo_path).unwrap();
    assert!(
        features_cargo.contains("nest-rs") && features_cargo.contains("graphql"),
        "the guards crate has to gain the graphql feature: {features_cargo}"
    );

    // A port with no entity stays the `#[public]` stand-in — nothing to guard.
    let resolver = fs::read_to_string(
        dir.path()
            .join("crates/features/src/posts/graphql/resolver.rs"),
    )
    .unwrap();
    assert!(resolver.contains("#[public]"), "{resolver}");
    assert!(
        !dir.path().join("crates/features/src/authz").exists(),
        "a workspace with no policy does not get one from a public count query",
    );
}

/// The GraphQL twin of `generate_resource_emits_the_guarded_form_…`: over a
/// `g resource` port the resolver is the `#[crud]` form, with the
/// `authz/graphql/` adapter it is enforced through.
#[test]
fn generate_graphql_over_a_resource_emits_the_crud_form_and_its_endpoint_guards() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "graphql", "posts", "-p", path]);

    let src = dir.path().join("crates/features/src");
    let resolver = fs::read_to_string(src.join("posts/graphql/resolver.rs")).unwrap();
    assert!(
        !resolver.contains("count()"),
        "a CrudService has no `count()`: {resolver}"
    );
    assert!(
        resolver.contains("#[use_guards(AuthzGuard)]"),
        "DB-backed rows are only reachable behind the ability guard: {resolver}"
    );
    // `AuthnGuard` has no `check_graphql`, so `#[resolver]` would refuse it.
    assert!(
        !resolver
            .lines()
            .any(|line| line.contains("use_guards(") && line.contains("AuthnGuard"))
            && !resolver.contains("use crate::authn::AuthnGuard"),
        "a resolver binds no guard without a GraphQL check: {resolver}"
    );
    assert!(
        resolver.contains("#[crud(") && resolver.contains("entity = PostEntity"),
        "the resource resolver uses the #[crud] form: {resolver}"
    );

    // The entity has to *be* a GraphQL object for the resolver to return it.
    let entity = fs::read_to_string(src.join("posts/entity.rs")).unwrap();
    assert!(entity.contains("#[expose(graphql"), "{entity}");

    // The guards `/graphql`'s request runs, and the module that serves it.
    let module = fs::read_to_string(src.join("posts/graphql/module.rs")).unwrap();
    assert!(module.contains("AuthzGraphqlModule"), "{module}");
    let authz_graphql = fs::read_to_string(src.join("authz/graphql/module.rs")).unwrap();
    assert!(
        authz_graphql.contains("EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)>")
            && authz_graphql.contains("forward_principal!(Claims)"),
        "the endpoint's guards, and the principal forward: {authz_graphql}"
    );
    assert!(
        !authz_graphql.contains("LoaderScope"),
        "a DataLoader batch carries what its request carries, with no binding: {authz_graphql}"
    );
    let authz_mod = fs::read_to_string(src.join("authz/mod.rs")).unwrap();
    assert!(
        authz_mod.contains("pub use graphql::AuthzGraphqlModule;"),
        "{authz_mod}"
    );

    // Its crates, with the features that make those paths resolve.
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    for needed in ["nest-rs", "async-graphql", "graphql"] {
        assert!(
            features_cargo.contains(needed),
            "{needed}: {features_cargo}"
        );
    }
    assert!(features_cargo.contains("authz"), "{features_cargo}");
}

/// A second GraphQL adapter reuses the authz adapter the first one created rather
/// than failing on a file that already exists.
#[test]
fn generate_graphql_reuses_an_existing_authz_adapter() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "graphql", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "resource", "tags", "-p", path]);
    run_ok(dir.path(), &["g", "graphql", "tags", "-p", path]);

    let authz_mod =
        fs::read_to_string(dir.path().join("crates/features/src/authz/mod.rs")).unwrap();
    assert_eq!(
        authz_mod.matches("pub mod graphql;").count(),
        1,
        "the index line is written once: {authz_mod}"
    );
}

/// `g graphql` also edits the app's `Cargo.toml`, so its printed next step
/// compiles.
#[test]
fn generate_graphql_gives_the_app_crate_the_dependency_its_next_step_needs() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "hello");
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "notes", "-p", root]);
    run_ok(&app, &["g", "graphql", "notes"]);

    let app_cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    assert!(
        app_cargo.contains("graphql"),
        "the app that has to import GraphqlModule needs the crate: {app_cargo}",
    );
}

#[test]
fn generate_adapter_is_rejected_on_rerun() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "http", "posts", "-p", path]);

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "http", "posts", "-p", path])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));
}

#[test]
fn generate_adapter_requires_existing_feature() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "ws", "ghost", "-p", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not found"));
}

/// `g ws` and `g mcp` write the authz adapter their output and code name.
#[test]
fn generate_ws_and_mcp_write_the_authz_adapters_their_own_output_names() {
    // (transport, authz dir, module, what only that adapter names)
    const CASES: [(&str, &str, &str, &str); 2] = [
        (
            "ws",
            "ws",
            "AuthzWsModule",
            "imports = [AuthzModule, WsModule]",
        ),
        (
            "mcp",
            "mcp",
            "AuthzMcpModule",
            "EndpointGuards<McpView, (AuthnGuard, AuthzGuard)>",
        ),
    ];

    for (transport, authz_dir, module, provider) in CASES {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        let app = write_fake_app(dir.path(), "api");
        let path = dir.path().to_str().unwrap();

        run_ok(dir.path(), &["g", "resource", "posts", "-p", path]);
        run_ok(&app, &["g", transport, "posts"]);

        let src = dir.path().join("crates/features/src");
        let authz_module = fs::read_to_string(src.join(format!("authz/{authz_dir}/module.rs")))
            .unwrap_or_else(|_| panic!("`g {transport}` writes authz/{authz_dir}/module.rs"));
        assert!(
            authz_module.contains(provider),
            "the {transport} authz adapter names {provider}: {authz_module}"
        );
        assert!(
            !authz_module.contains("DataContext"),
            "the edge opens its own units of work; no data context is listed: {authz_module}"
        );

        // An unimported `AuthzMcpModule` leaves `/mcp` deny-all.
        let authz_mod = fs::read_to_string(src.join("authz/mod.rs")).unwrap();
        assert!(
            authz_mod.contains(&format!("pub use {authz_dir}::{module};")),
            "{authz_mod}"
        );
        let app_module = fs::read_to_string(app.join("src/module.rs")).unwrap();
        assert!(
            app_module.contains(module),
            "the app composes {module}: {app_module}"
        );
    }
}

/// The authz adapter is scaffolded only where there is a policy to enforce: over a
/// `g feature` port with no auth adapter, `g ws` writes the adapter and stops.
#[test]
fn generate_ws_without_an_auth_adapter_writes_no_authz_adapter() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    let src = dir.path().join("crates/features/src");
    assert!(
        src.join("posts/ws/gateway.rs").is_file(),
        "the adapter lands"
    );
    assert!(
        !src.join("authz/ws").exists(),
        "no policy to enforce, so no authz adapter",
    );
}

/// Over a CRUD port with no auth adapter, `g graphql` bootstraps one as
/// `g resource` does, and announces it the same way: secrets included.
#[test]
fn generate_graphql_over_a_crud_port_announces_the_auth_adapter_it_bootstraps() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    let service = dir.path().join("crates/features/src/posts/service.rs");
    let crud = format!(
        "{}\nimpl CrudService for PostsService {{}}\n",
        fs::read_to_string(&service).unwrap()
    );
    fs::write(&service, crud).unwrap();

    let printed = run_ok(dir.path(), &["g", "graphql", "posts", "-p", path]);

    assert!(
        dir.path()
            .join("crates/features/src/authz/ability.rs")
            .is_file()
    );
    assert!(
        printed.contains("Also created the auth adapter"),
        "{printed}"
    );
    let var = scaffolded_var("AUTHN", "SECRET");
    for file in [".env.local", ".env.test"] {
        let secret = assigned(&dir.path().join(file), &var).expect("a secret of its own");
        assert!(!printed.contains(&secret), "{printed}");
        assert!(
            printed.contains(&format!("`{file}` holds")),
            "the run says where the secret is:\n{printed}",
        );
    }
}
