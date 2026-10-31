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

/// A generated route is guarded: no posture of its own, the two guards on its
/// controller, and the auth adapter they come from bootstrapped when the
/// workspace has none.
#[test]
fn generate_http_adapter_binds_the_guards_and_bootstraps_their_auth() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    let printed = run_ok(dir.path(), &["g", "http", "posts", "-p", path]);

    let src = dir.path().join("crates/features/src");
    let controller = fs::read_to_string(src.join("posts/http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[use_guards(AuthnGuard, AuthzGuard)]"),
        "the scaffolded route is guarded: {controller}"
    );
    assert!(
        !controller.lines().any(|line| line.trim() == "#[public]"),
        "an opening is the developer's line, never the generator's: {controller}"
    );
    let module = fs::read_to_string(src.join("posts/http/module.rs")).unwrap();
    assert!(module.contains("AuthzModule"), "{module}");
    assert!(
        src.join("authz/ability.rs").is_file(),
        "the guards' adapter lands"
    );
    assert!(
        printed.contains("Also created the auth adapter"),
        "{printed}"
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
            && module_rs.contains("imports = [PostsModule, HttpModule, WsModule"),
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

/// A typed WS payload reaches its derives through `#[input]`, so the adapter
/// adds a feature and not a `serde` entry; the auth adapter, written first,
/// brings its own.
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
    run_ok(dir.path(), &["g", "auth", "-p", path]);
    let serde_lines = |manifest: &str| manifest.lines().filter(|l| l.contains("serde")).count();
    let before = serde_lines(&fs::read_to_string(&features_cargo_path).unwrap());
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    let features_cargo = fs::read_to_string(&features_cargo_path).unwrap();
    assert_eq!(serde_lines(&features_cargo), before, "{features_cargo}");
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
        module_rs.contains("imports = [PostsModule, QueueModule]"),
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

/// A headless workspace (`default-features = false`) has no `http` until an
/// HTTP adapter asks for it; the entity before it turns on `seaorm` alone.
#[test]
fn generate_http_adapter_enables_the_http_feature_on_a_headless_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root_manifest = dir.path().join("Cargo.toml");
    let headless = fs::read_to_string(&root_manifest).unwrap()
        + "\n[workspace.dependencies]\n\
           nest-rs = { version = \"7.0\", default-features = false, features = [\"config\"] }\n";
    fs::write(&root_manifest, headless).unwrap();
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", root]);
    run_ok(dir.path(), &["g", "entity", "posts", "-p", root]);
    run_ok(dir.path(), &["g", "http", "posts", "-p", root]);

    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        features_cargo.contains("\"http\""),
        "the controller imports `nest_rs::http`, which the umbrella gates on `http`: \
         {features_cargo}",
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

    // A port with no entity gates its operation on the `#[subject]` written
    // beside its service, behind the guards its auth adapter brings.
    let src = dir.path().join("crates/features/src");
    let resolver = fs::read_to_string(src.join("posts/graphql/resolver.rs")).unwrap();
    assert!(
        resolver.contains("#[authorize(Read, Post)]")
            && resolver.contains("#[use_guards(AuthzGuard)]"),
        "{resolver}"
    );
    assert!(
        !resolver.lines().any(|line| line.trim() == "#[public]"),
        "an opening is the developer's line, never the generator's: {resolver}"
    );
    let subject = fs::read_to_string(src.join("posts/subject.rs")).unwrap();
    assert!(
        subject.contains("#[subject]") && subject.contains("pub struct Post;"),
        "{subject}"
    );
    let mod_rs = fs::read_to_string(src.join("posts/mod.rs")).unwrap();
    assert!(
        mod_rs.contains("mod subject;") && mod_rs.contains("pub use subject::Post;"),
        "{mod_rs}"
    );
    let module = fs::read_to_string(src.join("posts/graphql/module.rs")).unwrap();
    assert!(module.contains("AuthzGraphqlModule"), "{module}");
    assert!(
        src.join("authz/ability.rs").is_file(),
        "the guards' adapter lands"
    );
    assert_fits_the_margin(&resolver);
}

/// A generated line, comments included, keeps within rustfmt's margin.
fn assert_fits_the_margin(source: &str) {
    for line in source.lines() {
        assert!(line.chars().count() <= 100, "{line}\n{source}");
    }
}

/// An MCP tool over a port with no entity is gated on the port's `#[subject]`,
/// written once whichever adapter comes first, and `/mcp` gets the guards that
/// attach the ability its gate reads.
#[test]
fn generate_mcp_over_a_feature_port_gates_the_tool_on_the_ports_subject() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "graphql", "posts", "-p", path]);
    let printed = run_ok(dir.path(), &["g", "mcp", "posts", "-p", path]);

    let src = dir.path().join("crates/features/src");
    let tool = fs::read_to_string(src.join("posts/mcp/tool.rs")).unwrap();
    assert!(tool.contains("#[authorize(Read, Post)]"), "{tool}");
    assert!(
        !tool.lines().any(|line| line.trim() == "#[public]"),
        "an opening is the developer's line, never the generator's: {tool}"
    );
    let module = fs::read_to_string(src.join("posts/mcp/module.rs")).unwrap();
    assert!(module.contains("AuthzMcpModule"), "{module}");
    let mod_rs = fs::read_to_string(src.join("posts/mod.rs")).unwrap();
    assert_eq!(
        mod_rs.matches("pub use subject::Post;").count(),
        1,
        "the second adapter reuses the subject: {mod_rs}"
    );
    assert!(
        printed.contains("ab.can(nest_rs::authz::Action::Read, crate::posts::Post)"),
        "the next step names the grant as `authz/ability.rs` resolves it: {printed}"
    );
    // A posture on a `#[subject]` masks nothing, so the tool says it is re-gated
    // on the entity before it answers with rows, never that its rows are masked.
    assert!(
        !tool.contains("arms the field-level mask")
            && tool.contains("re-gated on that entity")
            && tool.contains("#[authorize(Read, PostEntity)]"),
        "{tool}"
    );
    assert_fits_the_margin(&tool);
}

/// Over a port that has an entity but no `CrudService`, the operation gates on
/// the entity, and no `#[subject]` is written beside it.
#[test]
fn generate_mcp_over_a_port_with_an_entity_gates_the_tool_on_the_entity() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "entity", "posts", "-p", path]);
    let printed = run_ok(dir.path(), &["g", "mcp", "posts", "-p", path]);

    let src = dir.path().join("crates/features/src");
    let tool = fs::read_to_string(src.join("posts/mcp/tool.rs")).unwrap();
    assert!(
        tool.contains("#[authorize(Read, PostEntity)]")
            && tool.contains("use crate::posts::{Entity as PostEntity, PostsService};"),
        "{tool}"
    );
    assert!(tool.contains("arms the field-level mask"), "{tool}");
    assert_fits_the_margin(&tool);
    assert!(!src.join("posts/subject.rs").exists());
    assert!(
        printed.contains("ab.can(nest_rs::authz::Action::Read, crate::posts::Entity)"),
        "the resolver's alias resolves nowhere in `authz/ability.rs`: {printed}"
    );
}

/// A port named by its own singular (`blog`) holds its subject in `subject.rs`
/// too: a file named for the subject would be `blog/blog.rs`, a module inside
/// one of the same name, which the scaffold's `clippy -D warnings` refuses.
#[test]
fn the_subject_of_a_port_named_by_its_own_singular_never_repeats_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "blog", "-p", path]);
    run_ok(dir.path(), &["g", "mcp", "blog", "-p", path]);

    let src = dir.path().join("crates/features/src");
    let subject = fs::read_to_string(src.join("blog/subject.rs")).unwrap();
    assert!(subject.contains("pub struct Blog;"), "{subject}");
    assert!(!src.join("blog/blog.rs").exists());
    let mod_rs = fs::read_to_string(src.join("blog/mod.rs")).unwrap();
    assert!(
        mod_rs.contains("pub use subject::Blog;") && !mod_rs.contains("mod blog;"),
        "{mod_rs}"
    );
}

/// Every file an adapter generator writes at a port passes the `nestrs lint`
/// the scaffold's `lint` and `ci` recipes run, whatever the port's plural:
/// `cities` names its types `City`, which only the singular of its folder
/// reaches.
#[test]
fn every_adapter_over_a_port_whose_plural_hides_its_singular_passes_the_lint() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "cities", "-p", path]);
    for transport in [
        "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
    ] {
        run_ok(dir.path(), &["g", transport, "cities", "-p", path]);
    }

    assert!(
        dir.path()
            .join("crates/features/src/cities/subject.rs")
            .is_file()
    );
    let linted = run_ok(dir.path(), &["lint", "-p", path]);
    assert!(linted.contains("named for what it declares"), "{linted}");
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
        resolver.contains("#[crud(service = svc)]"),
        "the resource resolver uses the #[crud] form, typed off its service: {resolver}"
    );

    // The entity has to *be* a GraphQL object for the resolver to return it.
    let entity = fs::read_to_string(src.join("posts/entity.rs")).unwrap();
    assert!(entity.contains("#[expose(graphql"), "{entity}");

    // The guards `/graphql`'s request runs, and the module that serves it.
    let module = fs::read_to_string(src.join("posts/graphql/module.rs")).unwrap();
    assert!(module.contains("AuthzGraphqlModule"), "{module}");
    let authz_graphql = fs::read_to_string(src.join("authz/graphql/module.rs")).unwrap();
    assert!(
        authz_graphql.contains("EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)>"),
        "the endpoint's guards: {authz_graphql}"
    );
    assert!(
        !authz_graphql.contains("forward_principal"),
        "a resolver reads the claims as a `Ctx<Claims>` parameter, with no forwarder: {authz_graphql}"
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

/// Over a `#[crud]` port the resolver holds an operation per action, so the next
/// step grants them all, on the entity's path as `authz/ability.rs` names it.
#[test]
fn generate_graphql_over_a_resource_prints_the_grant_its_crud_operations_wait_for() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "articles", "-p", path]);
    let printed = run_ok(dir.path(), &["g", "graphql", "articles", "-p", path]);

    assert!(
        printed.contains("ab.can(nest_rs::authz::Action::Manage, crate::articles::Entity)"),
        "{printed}"
    );
    assert!(
        !printed.contains("ArticleEntity") && !printed.contains("Action::Read"),
        "a single read grant leaves the writes FORBIDDEN: {printed}"
    );
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

/// Every adapter imports the modules that serve its edge, bare, so the app
/// importing the adapter serves it and is told nothing more to import; an
/// event's bus is imported by the port that emits it.
#[test]
fn every_adapter_imports_the_modules_that_serve_its_edge() {
    // (transport, the file importing them, below the feature, each `use` path)
    const CASES: [(&str, &str, &[&str]); 7] = [
        ("http", "http/module.rs", &["nest_rs::http::HttpModule"]),
        (
            "graphql",
            "graphql/module.rs",
            &["nest_rs::graphql::GraphqlModule"],
        ),
        (
            "ws",
            "ws/module.rs",
            &["nest_rs::http::HttpModule", "nest_rs::ws::WsModule"],
        ),
        ("queue", "queue/module.rs", &["nest_rs::queue::QueueModule"]),
        (
            "schedule",
            "schedule/module.rs",
            &["nest_rs::schedule::ScheduleModule"],
        ),
        ("mcp", "mcp/module.rs", &["nest_rs::http::HttpModule"]),
        ("events", "module.rs", &["nest_rs::events::EventsModule"]),
    ];

    for (transport, file, modules) in CASES {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        let app = write_fake_app(dir.path(), "api");
        let root = dir.path().to_str().unwrap();

        run_ok(dir.path(), &["g", "feature", "posts", "-p", root]);
        let printed = run_ok(&app, &["g", transport, "posts"]);

        let module_rs =
            fs::read_to_string(dir.path().join("crates/features/src/posts").join(file)).unwrap();
        for module in modules {
            let ident = module.rsplit("::").next().unwrap();
            let imports = module_rs
                .split("imports = [")
                .nth(1)
                .and_then(|rest| rest.split(']').next())
                .unwrap_or_default();
            let listed = imports.split([',', ' ', '\n']).any(|token| token == ident);
            assert!(
                module_rs.contains(&format!("use {module};")) && listed,
                "`g {transport}` imports {ident} in posts/{file}: {module_rs}",
            );
        }
        assert!(
            !printed.contains("Make sure the app imports"),
            "`g {transport}` wired what serves its edge itself: {printed}",
        );
    }
}

/// The queue backend is the app's choice, so `g queue` names its binding when
/// the app binds none, and only then.
#[test]
fn generate_queue_names_the_backend_binding_only_when_the_app_binds_none() {
    const UNBOUND: &str = "binds no queue backend";

    for bound in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        let app = write_fake_app(dir.path(), "worker");
        if bound {
            let module_rs = app.join("src/module.rs");
            let source = fs::read_to_string(&module_rs).unwrap().replace(
                "        HttpModule,",
                "        HttpModule,\n        RedisQueueModule,",
            );
            fs::write(&module_rs, source).unwrap();
        }
        let root = dir.path().to_str().unwrap();

        run_ok(dir.path(), &["g", "feature", "audio", "-p", root]);
        let printed = run_ok(&app, &["g", "queue", "audio"]);

        assert_eq!(
            printed.contains(UNBOUND),
            !bound,
            "the binding is named only where the app has none: {printed}",
        );
        let app_cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
        assert!(
            app_cargo.contains("redis-queue"),
            "the app that binds the backend needs the binding's feature: {app_cargo}",
        );
    }
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

/// `g mcp` writes the authz adapter its output and code name, which the
/// generated adapter imports and the app does not.
#[test]
fn generate_mcp_writes_the_authz_adapter_its_own_output_names() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "posts", "-p", path]);
    run_ok(&app, &["g", "mcp", "posts"]);

    let src = dir.path().join("crates/features/src");
    let authz_module = fs::read_to_string(src.join("authz/mcp/module.rs"))
        .expect("`g mcp` writes authz/mcp/module.rs");
    assert!(
        authz_module.contains("EndpointGuards<McpView, (AuthnGuard, AuthzGuard)>"),
        "the MCP authz adapter declares the endpoint's guards: {authz_module}"
    );
    assert!(
        !authz_module.contains("DataContext"),
        "the edge opens its own units of work; no data context is listed: {authz_module}"
    );

    // An unimported `AuthzMcpModule` leaves `/mcp` deny-all.
    let authz_mod = fs::read_to_string(src.join("authz/mod.rs")).unwrap();
    assert!(
        authz_mod.contains("pub use mcp::AuthzMcpModule;"),
        "{authz_mod}"
    );
    let adapter = fs::read_to_string(src.join("posts/mcp/module.rs")).unwrap();
    assert!(
        adapter.contains("AuthzMcpModule"),
        "the adapter imports AuthzMcpModule: {adapter}"
    );
    let app_module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(
        !app_module.contains("AuthzMcpModule"),
        "the app lists the adapter, not what it brings: {app_module}"
    );
}

/// A generated gateway is guarded at its upgrade: over a `g feature` port with
/// no auth adapter, `g ws` bootstraps it, and the gateway's adapter imports
/// `AuthzModule`, as an HTTP adapter does — no WebSocket authz adapter of its
/// own.
#[test]
fn generate_ws_over_a_feature_port_guards_the_upgrade_and_bootstraps_its_auth() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "ws", "posts", "-p", path]);

    let src = dir.path().join("crates/features/src");
    let gateway = fs::read_to_string(src.join("posts/ws/gateway.rs")).unwrap();
    assert!(
        gateway.contains("#[use_guards(AuthnGuard, AuthzGuard)]"),
        "{gateway}"
    );
    let module = fs::read_to_string(src.join("posts/ws/module.rs")).unwrap();
    assert!(module.contains("AuthzModule"), "{module}");
    assert!(
        !src.join("authz/ws").exists(),
        "a gateway reuses the HTTP pair, through AuthzModule"
    );
}

/// A `#[crud]` controller or resolver runs each call in the unit of work
/// `SeaOrmDatabaseModule` binds, and an app without it is refused at boot: over
/// a CRUD port, `g http` and `g graphql` wire only into an app importing the
/// binding, as `g resource` does, and otherwise name it in the next step.
#[test]
fn a_crud_adapter_is_wired_only_into_an_app_binding_the_unit_of_work() {
    for (transport, module) in [
        ("http", "PostsHttpModule"),
        ("graphql", "PostsGraphqlModule"),
    ] {
        for bound in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            write_fake_workspace(dir.path());
            let app = write_fake_app(dir.path(), "api");
            let path = dir.path().to_str().unwrap();
            run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
            let service = dir.path().join("crates/features/src/posts/service.rs");
            let crud = format!(
                "{}\nimpl CrudService for PostsService {{}}\n",
                fs::read_to_string(&service).unwrap()
            );
            fs::write(&service, crud).unwrap();
            let module_rs = app.join("src/module.rs");
            if bound {
                let with_db = fs::read_to_string(&module_rs).unwrap().replace(
                    "    ],",
                    "        SeaOrmModule,\n        SeaOrmDatabaseModule,\n    ],",
                );
                fs::write(&module_rs, with_db).unwrap();
            }

            let printed = run_ok(&app, &["g", transport, "posts"]);

            let composed = fs::read_to_string(&module_rs).unwrap();
            assert_eq!(
                composed.contains(module),
                bound,
                "`g {transport}` wires {module} only beside the binding: {composed}"
            );
            assert_eq!(
                printed.contains("Add `SeaOrmModule` and `SeaOrmDatabaseModule` to this app"),
                !bound,
                "the next step names the binding the app lacks: {printed}"
            );
        }
    }
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
