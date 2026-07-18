use std::fs;
use std::path::Path;
use std::process::Command;

fn write_fake_workspace(root: &Path) {
    fs::create_dir_all(root.join("crates/features/src")).unwrap();
    fs::create_dir_all(root.join("apps")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        r#"
[workspace]
members = ["crates/*", "apps/*"]

[workspace.package]
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        root.join("crates/features/src/lib.rs"),
        "pub mod users;\n\npub use users::UsersModule;\n",
    )
    .unwrap();
    fs::write(
        root.join("crates/features/Cargo.toml"),
        "[package]\nname = \"features\"\n\n[dependencies]\nnest-rs-core.workspace = true\n",
    )
    .unwrap();
}

/// Run `nestrs <args...>` with cwd at `dir`, asserting success.
fn run_ok(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "args {args:?} stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn new_standalone_hello_template() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "demo-api", "--standalone"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("demo-api");
    assert!(app.join("src/main.rs").is_file());
    assert!(app.join("src/lib.rs").is_file());
    assert!(app.join("src/controller.rs").is_file());
    assert!(app.join("tests/e2e/main.rs").is_file());
    assert!(app.join("Cargo.toml").is_file());
    assert!(app.join("Dockerfile").is_file());
    assert!(app.join(".dockerignore").is_file());
    assert!(app.join("rust-toolchain.toml").is_file());
    assert!(app.join(".env").is_file());
    assert!(app.join(".env.development").is_file());
    let dev_env = fs::read_to_string(app.join(".env.development")).unwrap();
    assert!(dev_env.contains("NESTRS_OPENTELEMETRY__LOG_LEVEL=debug"));
    assert!(app.join(".env.example").is_file());

    let main_rs = fs::read_to_string(app.join("src/main.rs")).unwrap();
    assert!(main_rs.contains("OpenTelemetry::init"));
    assert!(main_rs.contains("Environment::init"));

    let module_rs = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module_rs.contains("OpenTelemetryModule"));

    let cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("[workspace]"));
    assert!(cargo.contains("nest-rs-guards"));
    assert!(cargo.contains("nest-rs-interceptors"));
    assert!(cargo.contains("nest-rs-opentelemetry"));
    assert!(app.join(".gitignore").is_file());
    assert!(app.join("Justfile").is_file());
    let justfile = fs::read_to_string(app.join("Justfile")).unwrap();
    assert!(justfile.contains("build:"));
    assert!(justfile.contains("cargo build --release"));
    assert!(justfile.contains("mod test"));
    assert!(justfile.contains("mod db"));
    assert!(app.join("test.just").is_file());
    let test_just = fs::read_to_string(app.join("test.just")).unwrap();
    assert!(test_just.contains("unit:"));
    assert!(test_just.contains("e2e:"));
    assert!(test_just.contains("doc:"));
    assert!(test_just.contains("cargo test --doc"));
    assert!(app.join("db.just").is_file());
    let db_just = fs::read_to_string(app.join("db.just")).unwrap();
    assert!(db_just.contains("up:"));
    assert!(db_just.contains("reset: fresh seed"));
}

#[test]
fn new_workspace_greenfield() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "acme"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let root = dir.path().join("acme");
    assert!(root.join("Cargo.toml").is_file());
    assert!(
        root.join("crates/features/src/hello/http/controller.rs")
            .is_file()
    );
    // The default app and demo feature are both named `hello`.
    assert!(root.join("apps/hello/src/module.rs").is_file());
    assert!(!root.join("apps/hello/src/controller.rs").exists());
    assert!(root.join("apps/hello/tests/e2e/main.rs").is_file());
    assert!(root.join("Justfile").is_file());
    let justfile = fs::read_to_string(root.join("Justfile")).unwrap();
    assert!(justfile.contains("dev app=\"hello\""));
    // `build --all` is a conditional on the single `build` recipe, not a separate recipe.
    assert!(!justfile.contains("build-all"));
    assert!(justfile.contains(r#"if app == "--all""#));
    assert!(justfile.contains("mod test"));
    assert!(justfile.contains("mod db"));

    let test_just = fs::read_to_string(root.join("test.just")).unwrap();
    assert!(test_just.contains("unit:"));
    assert!(test_just.contains("e2e:"));
    assert!(test_just.contains("cargo test --workspace --doc"));
    let db_just = fs::read_to_string(root.join("db.just")).unwrap();
    assert!(db_just.contains("up:"));
    assert!(db_just.contains("reset: fresh seed"));

    let module = fs::read_to_string(root.join("apps/hello/src/module.rs")).unwrap();
    assert!(module.contains("HelloHttpModule"));
    assert!(module.contains("features::hello"));
    assert!(module.contains("port: 3000"));

    let env = fs::read_to_string(root.join(".env")).unwrap();
    assert!(!env.contains("NESTRS_HTTP__PORT"));

    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("members = [\"crates/*\", \"apps/*\"]"));
}

#[test]
fn new_app_inside_nestrs_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "demo-api"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("apps/demo-api");
    assert!(app.join("src/module.rs").is_file());
    assert!(app.join("src/main.rs").is_file());
    assert!(!app.join("src/controller.rs").exists());

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(!module.contains("for_root(None)"));
}

#[test]
fn new_app_picks_next_http_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    fs::create_dir_all(dir.path().join("apps/auth/src")).unwrap();
    fs::write(
        dir.path().join("apps/auth/src/module.rs"),
        "HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() })",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "blog"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let module = fs::read_to_string(dir.path().join("apps/blog/src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3002"));
}

#[test]
fn new_app_inside_workspace_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    fs::create_dir_all(dir.path().join("apps/blog")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "blog"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already exists"));
    assert!(stderr.contains("blog"));
}

#[test]
fn new_workspace_app_scaffold() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "blog", "-o"])
        .arg(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("apps/blog");
    assert!(app.join("src/module.rs").is_file());
    assert!(app.join("src/lib.rs").is_file());
    assert!(!app.join("src/controller.rs").exists());
    assert!(dir.path().join(".env").is_file());
    assert!(dir.path().join(".env.development").is_file());
    assert!(dir.path().join("Justfile").is_file());
    assert!(dir.path().join(".gitignore").is_file());
    assert!(dir.path().join("compose.yml").is_file());

    // The committed `.env` points at the compose services (T26): the DB URL is
    // active so `nestrs run db up` works out of the box; the port stays code.
    let env = fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(!env.contains("NESTRS_HTTP__PORT"));
    assert!(env.contains("NESTRS_DATABASE__URL=postgres://"));

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(module.contains("OpenTelemetryModule"));
}

#[test]
fn generate_feature_creates_port_and_wires_lib() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &["g", "feature", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("mod.rs").is_file());
    assert!(feature.join("module.rs").is_file());
    assert!(feature.join("service.rs").is_file());
    // no transport yet
    assert!(!feature.join("http").exists());

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}

#[test]
fn generate_resource_creates_crud_slice_and_deps() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &["g", "resource", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("entity.rs").is_file());
    assert!(feature.join("service.rs").is_file());
    assert!(feature.join("http/controller.rs").is_file());

    let entity = fs::read_to_string(feature.join("entity.rs")).unwrap();
    assert!(entity.contains("#[expose(name = \"Post\""));
    assert!(entity.contains("table_name = \"post\""));

    // dependencies spliced into both manifests
    let root_cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(root_cargo.contains("nest-rs-seaorm"));
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("nest-rs-seaorm"));

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}

#[test]
fn generate_resource_guarded_emits_the_crud_and_guards_form() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &[
            "g",
            "resource",
            "posts",
            "--guarded",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    let feature = dir.path().join("crates/features/src/posts");
    let controller = fs::read_to_string(feature.join("http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[use_guards(AuthGuard, AuthzGuard)]"),
        "guarded controller binds the guards: {controller}"
    );
    assert!(
        controller.contains("#[crud(") && controller.contains("entity = PostEntity"),
        "guarded controller uses the #[crud] form: {controller}"
    );
    // The unguarded stub's SECURITY comment must be gone.
    assert!(!controller.contains("scaffolded without guards"));

    let module = fs::read_to_string(feature.join("http/module.rs")).unwrap();
    assert!(
        module.contains("AuthzHttpModule"),
        "guarded http module imports AuthzHttpModule: {module}"
    );
}

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
    assert!(features_cargo.contains("nest-rs-ws"));
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
    assert!(processor_rs.contains("use crate::posts::ProcessPostCommand;"));
    assert!(processor_rs.contains("job: ProcessPostCommand"));
    // The payload is imported, never redefined in the adapter.
    assert!(!processor_rs.contains("pub struct ProcessPostCommand"));

    // The port `mod.rs` exposes both the command and the adapter module.
    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("mod command;"));
    assert!(mod_rs.contains("pub use command::ProcessPostCommand;"));
    assert!(mod_rs.contains("pub mod queue;"));
    assert!(mod_rs.contains("PostsQueueModule"));
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

#[test]
fn generate_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &[
            "g",
            "feature",
            "posts",
            "--dry-run",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(!dir.path().join("crates/features/src/posts").exists());
    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(!lib.contains("pub mod posts;"));
}

#[test]
fn version_prints_single_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("version")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();
    assert!(line.starts_with("NestRS "));
    assert!(!line.contains('\n'));
}

#[test]
fn about_prints_metadata_block() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("about")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Version:"));
    assert!(stdout.contains("Tagline:"));
    assert!(stdout.contains("Yoann Vanitou"));
}

#[test]
fn run_subcommand_is_listed_in_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("run"));
}

#[test]
fn run_without_toolchain_and_no_bootstrap_errors_clearly() {
    // Hide just/bacon/cargo from the child so the toolchain probe finds nothing,
    // then assert the bootstrap-disabled path reports a manual-install hint
    // instead of silently installing or panicking.
    let empty = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["run", "--no-bootstrap", "dev"])
        .env("PATH", empty.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("just"), "stderr: {stderr}");
    assert!(stderr.contains("cargo install"), "stderr: {stderr}");
}

#[test]
fn doctor_passes_with_rust_toolchain() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("doctor")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
