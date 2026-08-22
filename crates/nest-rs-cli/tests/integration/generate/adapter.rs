//! `nestrs g http|graphql|ws|queue|schedule|mcp` — one uniform generator, so the
//! obligations are per transport: the right template, the crates its expansion
//! names, and the wiring into feature and app.

use crate::harness::{run_ok, write_fake_app, write_fake_workspace};
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
    assert!(mod_rs.contains("PostsController"));
    assert!(mod_rs.contains("PostsHttpModule"));
}

/// Every route declares a posture — the `hello` starter and the GraphQL adapter
/// both write `#[public]`, and the HTTP adapter used to write neither, so any
/// app that took a `g http` booted with `unguarded routes detected` on a route
/// nobody had decided about.
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
/// one. The template hard-coded `/ws`, which meant a second `g ws` produced an
/// app that failed boot on `duplicate self-mounted endpoint path "/ws"` — and
/// the generator's own next-step line told you to wire it in.
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

    // The HTTP adapter claims `/<feature>`; the gateway must not land there
    // either — a controller prefix and a self-mount on one path is the same
    // exclusivity failure across families.
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
/// reads. Left out, the app compiles, mounts the gateway, and *then* dies — so
/// the generator writes the import rather than leaving it to the boot.
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

/// `#[messages]` expands to `nest_rs_guards::GuardAsWsMessageCheck`, gated
/// behind that crate's `ws` feature — and the gateway body logs. Without both,
/// `cargo check -p features` fails while `cargo check --workspace` passes
/// (feature unification through a dev-dependency), which is the worst possible
/// place for the failure to surface.
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

/// A typed WS payload reaches its derives through `#[input]`, so the generated
/// manifest gains a feature and not a `serde` entry — the scaffold must not
/// re-introduce a line the decorator exists to absorb.
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

/// A2: the `ws` skeleton hardcoded `self.svc.count()`, which a `g resource`
/// port's `CrudService` does not have — so the CLI page's "any adapter compiles
/// immediately" guarantee broke, with rustc blaming `Iterator::count`.
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

/// The schedule skeleton logs too — same class as the ws and queue ones.
#[test]
fn generate_schedule_adapter_brings_tracing() {
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
    assert!(tasks_rs.contains("tracing::"), "the skeleton logs");
    // …and carries a field while doing it: a bare event is a defect the
    // generator would ship into every project (`templates::tests` pins this for
    // every template; this is the end-to-end half).
    assert!(
        tasks_rs.contains(r#"every = "60s""#),
        "the scaffolded tick must log with a structured field: {tasks_rs}",
    );
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        features_cargo.contains("tracing"),
        "…so `tracing` has to be a dependency: {features_cargo}"
    );
}

#[test]
fn generate_queue_adapter_puts_command_at_the_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "queue", "posts", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");

    // The imperative queue payload is a `Command` at the port, not inside the
    // `queue/` adapter — a producer↔worker contract the processor imports.
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
    // The payload is imported, never redefined in the adapter.
    assert!(!processor_rs.contains("pub struct ProcessPostCommand"));

    // The port `mod.rs` exposes both the command and the adapter module.
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

/// The `#[queue]` marker is the artifact `push_to::<Q>` is generic over, so it
/// has to be *reachable*. Declared in the adapter's private `processor` module
/// it was invisible to the feature's own service one directory up, leaving the
/// untyped `push(name, job)` escape hatch as the only way to enqueue — the exact
/// check `QueueName` exists to provide, lost.
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

    // Reachable from the port, which is what a producer imports.
    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("PostsQueue"), "{mod_rs}");
}

/// The generated processor module imports the port like every sibling adapter
/// does. It reads as redundant only while the stub stays inert: give it the
/// documented shape — a thin processor delegating to the port service — and
/// without the import the worker dies at boot with an access violation.
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

    // The skeleton logs, so `tracing` has to come with it.
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("tracing"), "{features_cargo}");
}

/// C1: `/queue/producing-jobs/` opens with `use nest_rs_redis::RedisQueueConnection;`
/// and the install stanza names the crate — the generator wrote four of the
/// five lines, so the first documented step after it did not compile.
#[test]
fn generate_queue_adapter_brings_the_connection_crate() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "audio", "-p", root]);
    run_ok(dir.path(), &["g", "queue", "audio", "-p", root]);

    let root_cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(root_cargo.contains("\"redis\""), "{root_cargo}");
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("\"redis\""), "{features_cargo}");
}

/// E6: the `mcp` feature is what seeds the fallback operation guard, without
/// which a registered global pool cannot gate `/mcp`.
///
/// (E1 — rmcp's `#[tool]` emitting bare `schemars::` paths, which used to force
/// a second manifest line on any tool taking input — is closed as of rmcp 3.x:
/// the input schema is built through `rmcp::handler::server::common`, so the
/// `use nest_rs::mcp::rmcp;` the template already writes covers it.
/// `nest-rs-macro-hygiene` compiles that exact shape with one dependency.)
#[test]
fn generate_mcp_adapter_brings_the_guard_fallback() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "feature", "tools", "-p", root]);
    run_ok(dir.path(), &["g", "mcp", "tools", "-p", root]);

    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        features_cargo.contains("nest-rs") && features_cargo.contains("\"mcp\""),
        "the mcp guard fallback rides the `mcp` feature: {features_cargo}",
    );
}

/// `#[resolver]` expands to names behind `nest-rs-guards`' `graphql` feature,
/// and that crate is already a dependency of every scaffolded workspace — so
/// the generator has to turn the *feature* on, not add the entry. Without it
/// the very first `cargo check` after `g graphql` is a wall of
/// `cannot find … in nest_rs_guards`.
#[test]
fn generate_graphql_adapter_enables_the_guards_graphql_feature() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    // The starter shape: the crate is declared, with default features.
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
/// `g resource` port the resolver is the `#[crud]` form behind the app's
/// guards, and the per-operation bridge it is enforced through
/// (`authz/graphql/`) is scaffolded with it. The old scaffold called
/// `svc.count()` — a method a `CrudService` does not have.
#[test]
fn generate_graphql_over_a_resource_emits_the_crud_form_and_its_bridge() {
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
        resolver.contains("#[use_guards(AuthnGuard, AuthzGuard)]"),
        "DB-backed rows are only reachable behind the ability guard: {resolver}"
    );
    assert!(
        resolver.contains("#[crud(") && resolver.contains("entity = PostEntity"),
        "the resource resolver uses the #[crud] form: {resolver}"
    );

    // The entity has to *be* a GraphQL object for the resolver to return it.
    let entity = fs::read_to_string(src.join("posts/entity.rs")).unwrap();
    assert!(entity.contains("#[expose(graphql"), "{entity}");

    // The bridge `/graphql` gates through, and the module that serves it.
    let module = fs::read_to_string(src.join("posts/graphql/module.rs")).unwrap();
    assert!(module.contains("AuthzGraphqlModule"), "{module}");
    let bridge = fs::read_to_string(src.join("authz/graphql/bridge.rs")).unwrap();
    assert!(
        bridge.contains("GraphqlAbilityBridge<AuthnGuard, AuthzGuard>"),
        "{bridge}"
    );
    let authz_graphql = fs::read_to_string(src.join("authz/graphql/module.rs")).unwrap();
    assert!(
        authz_graphql.contains("dyn GraphqlOperationGuard")
            && authz_graphql.contains("dyn GraphqlBatchContext")
            && authz_graphql.contains("forward_principal!(Claims)"),
        "the two providers the bridge needs, and the principal forward: {authz_graphql}"
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

/// A second GraphQL adapter reuses the bridge the first one created rather
/// than failing on a file that already exists.
#[test]
fn generate_graphql_reuses_an_existing_authz_bridge() {
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

/// D1: `g graphql` edited the app's `module.rs` and left its `Cargo.toml`
/// alone, so the generator's own printed next step (`use
/// nest_rs_graphql::GraphqlModule;`) failed with `E0433`.
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

/// F4: `g ws` and `g mcp` used to *name* an authz module in their output and in
/// the code they generated — `AuthzWsModule` in the gateway's SECURITY comment,
/// `features::authz::mcp` in the MCP next step and the tool's own doc — while no
/// generator wrote either. A reader following the docs end to end got a boot
/// `warn` on an unguarded self-mount edge, a `/mcp` answering 401 to everything,
/// and the repo's demo as the only source for the two modules.
///
/// One case per transport, in one test: the obligation is identical, and it is
/// the *set* of transports that carries it which regressed.
#[test]
fn generate_ws_and_mcp_write_the_authz_bridges_their_own_output_names() {
    // (transport, bridge dir, module, a provider only that bridge registers)
    const CASES: [(&str, &str, &str, &str); 2] = [
        ("ws", "ws", "AuthzWsModule", "dyn SocketContext"),
        ("mcp", "mcp", "AuthzMcpModule", "dyn McpOperationGuard"),
    ];

    for (transport, bridge_dir, module, provider) in CASES {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        let app = write_fake_app(dir.path(), "api");
        let path = dir.path().to_str().unwrap();

        run_ok(dir.path(), &["g", "resource", "posts", "-p", path]);
        // Run from inside the app, so the composition site is wired too.
        run_ok(&app, &["g", transport, "posts"]);

        let src = dir.path().join("crates/features/src");
        let bridge_module = fs::read_to_string(src.join(format!("authz/{bridge_dir}/module.rs")))
            .unwrap_or_else(|_| panic!("`g {transport}` writes authz/{bridge_dir}/module.rs"));
        assert!(
            bridge_module.contains(provider),
            "the {transport} bridge registers {provider}: {bridge_module}"
        );

        // Reachable from the feature crate's root, and from the app that serves
        // the adapter — an unimported AuthzMcpModule leaves /mcp deny-all.
        let authz_mod = fs::read_to_string(src.join("authz/mod.rs")).unwrap();
        assert!(
            authz_mod.contains(&format!("pub use {bridge_dir}::{module};")),
            "{authz_mod}"
        );
        let app_module = fs::read_to_string(app.join("src/module.rs")).unwrap();
        assert!(
            app_module.contains(module),
            "the app composes {module}: {app_module}"
        );
    }
}

/// The bridge is scaffolded only where there is a policy to enforce. A `g
/// feature` port in a workspace with no auth adapter has none, so `g ws` writes
/// the adapter and stops — scaffolding a whole authn/authz slice off the back of
/// a WebSocket stub would be the generator deciding something the developer has
/// not.
#[test]
fn generate_ws_without_an_auth_adapter_writes_no_bridge() {
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
        "no policy to bridge, so no bridge",
    );
}
