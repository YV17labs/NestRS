use std::path::Path;

use crate::commands::queue_db_crates;
use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::port::next_http_port;
use crate::scaffold::{Renderer, Scaffold, ensure_lines, rustfmt};
use crate::templates::{hello, shared, workspace};

use super::queue_env_files;

const HELLO_APP_PORT: u16 = 3000;

pub fn scaffold_root(output: &Path, names: &Names, dry_run: bool) -> CliResult<()> {
    let root = output.join(&names.kebab);
    if root.exists() {
        return Err(CliError::AlreadyExists(root));
    }

    let r = Renderer::new(names);
    let mut s = Scaffold::new();

    s.create(root.join("Cargo.toml"), r.render(workspace::ROOT_CARGO));
    s.create(
        root.join("rust-toolchain.toml"),
        r.render(shared::RUST_TOOLCHAIN),
    );
    s.create(root.join("README.md"), r.render(workspace::README));
    s.create(
        root.join("crates/features/Cargo.toml"),
        r.render(workspace::FEATURES_CARGO),
    );
    queue_db_crates(&mut s, &root, &[]);
    queue_root_files(&mut s, &root, names);

    // The first app is always `hello`, whatever the workspace is called.
    let hello_names = Names::parse("hello");
    let hello_r = Renderer::new(&hello_names);
    s.create(
        root.join("crates/features/src/lib.rs"),
        hello_r.render(workspace::FEATURES_LIB),
    );
    queue_hello_feature(
        &mut s,
        &root.join("crates/features/src").join(&hello_names.snake),
        &hello_names,
    );
    queue_app(
        &mut s,
        &root.join("apps").join(&hello_names.kebab),
        &hello_names,
        HELLO_APP_PORT,
    );

    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }

    println!("Created nestrs workspace at {}", root.display());
    report.print(output);
    print_root_next_steps(&root);
    Ok(())
}

pub fn scaffold_app(ws: &NestrsWorkspace, names: &Names, dry_run: bool) -> CliResult<()> {
    let root = ws.apps_root().join(&names.kebab);
    if root.exists() {
        return Err(CliError::AppExists {
            name: names.kebab.clone(),
            path: root,
        });
    }
    // Same shape as the root scaffold: the app gets a `hello` feature named
    // after it, so `nestrs run dev <app>` answers on `/` the first time. A
    // feature already owning that name would be clobbered, so refuse instead —
    // the app name is the one thing the caller can change for free.
    if ws.feature_exists(&names.snake) {
        return Err(CliError::FeatureExists {
            name: names.snake.clone(),
            path: ws.feature_root(&names.snake),
        });
    }

    let port = next_http_port(ws)?;
    let mut s = Scaffold::new();
    queue_hello_feature(&mut s, &ws.feature_root(&names.snake), names);
    s.edit(
        ws.features_lib(),
        ensure_lines(vec![
            format!("pub mod {};", names.snake),
            format!("pub use {}::{};", names.snake, names.http_module()),
        ]),
    );
    queue_app(&mut s, &root, names, port);
    queue_root_files(&mut s, &ws.root, names);

    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }

    println!(
        "Created workspace app `{}` at {}",
        names.kebab,
        root.display()
    );
    println!("HTTP port: {port} (pinned in src/module.rs)");
    report.print(&ws.root);
    print_app_next_steps(ws, names, port);
    Ok(())
}

/// The app's `hello` feature — port (module + service) plus its HTTP adapter,
/// the shared [`hello`] templates in the workspace's feature layout. Every
/// identifier derives from `names`, so `hello` and any later app are one shape.
fn queue_hello_feature(s: &mut Scaffold, feature_root: &Path, names: &Names) {
    let r = Renderer::new(names).with(
        "service_use",
        format!("crate::{}::{}", names.snake, names.service()),
    );
    s.create(feature_root.join("mod.rs"), r.render(hello::FEATURE_MOD));
    s.create(
        feature_root.join("module.rs"),
        r.render(hello::FEATURE_MODULE),
    );
    s.create(feature_root.join("service.rs"), r.render(hello::SERVICE));
    s.create(
        feature_root.join("http/mod.rs"),
        r.render(hello::FEATURE_HTTP_MOD),
    );
    s.create(
        feature_root.join("http/module.rs"),
        r.render(hello::FEATURE_HTTP_MODULE),
    );
    s.create(
        feature_root.join("http/controller.rs"),
        r.render(hello::CONTROLLER),
    );
}

fn queue_app(s: &mut Scaffold, app_root: &Path, names: &Names, port: u16) {
    let r = Renderer::new(names).with("port", port.to_string());
    s.create(app_root.join("Cargo.toml"), r.render(workspace::APP_CARGO));
    s.create(app_root.join("src/lib.rs"), r.render(workspace::APP_LIB));
    s.create(app_root.join("src/main.rs"), r.render(workspace::APP_MAIN));
    s.create(
        app_root.join("src/module.rs"),
        r.render(workspace::APP_MODULE),
    );

    // No live infra involved ⇒ `integration`, never `e2e` (the suite norm).
    s.create(
        app_root.join("tests/integration/main.rs"),
        r.render(shared::SMOKE),
    );
    // Empty, but present: the test recipes filter on `binary(e2e)`, which
    // nextest refuses to parse when no such binary exists.
    s.create(app_root.join("tests/e2e/main.rs"), r.render(shared::E2E));
}

fn queue_root_files(s: &mut Scaffold, base: &Path, names: &Names) {
    let r = Renderer::new(names);
    queue_env_files(s, base, names, "nestrs workspace", shared::ENV_WORKSPACE);
    s.create_if_missing(base.join("Justfile"), r.render(workspace::JUSTFILE));
    s.create_if_missing(base.join("test.just"), r.render(workspace::TEST_JUSTFILE));
    s.create_if_missing(base.join("db.just"), r.render(shared::DB_JUSTFILE));
    s.create_if_missing(base.join("compose.yml"), r.render(shared::COMPOSE));
    s.create_if_missing(base.join(".gitignore"), r.render(shared::GITIGNORE));
    // No `.dockerignore`: workspace mode ships no Dockerfile, so there is
    // nothing for it to scope. Standalone mode, which does, writes one.
}

fn print_root_next_steps(root: &Path) {
    println!();
    println!("Mode: workspace (crates/features/ + apps/*)");
    println!();
    println!("Next steps:");
    println!("  cd {}", root.display());
    println!("  nestrs run dev hello");
    println!("  Open http://localhost:{HELLO_APP_PORT}/ in your browser");
    println!();
    println!("Add another app:  nestrs new <name>");
    println!("Add a feature:    nestrs g feature <name>   (then g http <name>)");
    println!("DB-backed CRUD:   nestrs g resource <name>");
}

fn print_app_next_steps(ws: &NestrsWorkspace, names: &Names, port: u16) {
    println!();
    println!("Next steps:");
    println!("  cd {}", ws.root.display());
    println!("  nestrs run dev {}", names.kebab);
    println!("  Open http://localhost:{port}/ in your browser");
}
