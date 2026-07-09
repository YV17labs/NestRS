use std::path::Path;

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::port::next_http_port;
use crate::scaffold::{Renderer, Scaffold, rustfmt};
use crate::templates::{shared, workspace};

use super::{NewTemplate, queue_env_files};

const HELLO_APP_PORT: u16 = 3000;

pub fn scaffold_root(
    output: &Path,
    names: &Names,
    template: NewTemplate,
    dry_run: bool,
) -> CliResult<()> {
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

    let hello = Names::parse("hello");
    queue_root_files(&mut s, &root, names);
    let app_dir = root.join("apps").join(&hello.kebab);

    match template {
        NewTemplate::Hello => {
            s.create(
                root.join("crates/features/src/lib.rs"),
                workspace::FEATURES_LIB_WITH_HELLO.to_string(),
            );
            queue_hello_feature(&mut s, &root.join("crates/features/src/hello"), &hello);
            queue_app(&mut s, &app_dir, &hello, true, HELLO_APP_PORT);
        }
        NewTemplate::Empty => {
            s.create(
                root.join("crates/features/src/lib.rs"),
                workspace::FEATURES_LIB.to_string(),
            );
            queue_app(&mut s, &app_dir, &hello, false, HELLO_APP_PORT);
        }
    }

    let report = s.apply(dry_run)?;
    if !dry_run {
        rustfmt(&report.rust_files());
    }

    println!("Created nestrs workspace at {}", root.display());
    println!("Template: {}", template.description());
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

    let port = next_http_port(ws)?;
    let mut s = Scaffold::new();
    queue_app(&mut s, &root, names, false, port);
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

fn queue_hello_feature(s: &mut Scaffold, feature_root: &Path, hello: &Names) {
    let r = Renderer::new(hello);
    s.create(feature_root.join("mod.rs"), r.render(workspace::HELLO_MOD));
    s.create(
        feature_root.join("module.rs"),
        r.render(workspace::HELLO_MODULE),
    );
    s.create(
        feature_root.join("service.rs"),
        r.render(workspace::HELLO_SERVICE),
    );
    s.create(
        feature_root.join("http/mod.rs"),
        r.render(workspace::HELLO_HTTP_MOD),
    );
    s.create(
        feature_root.join("http/module.rs"),
        r.render(workspace::HELLO_HTTP_MODULE),
    );
    s.create(
        feature_root.join("http/controller.rs"),
        r.render(workspace::HELLO_HTTP_CONTROLLER),
    );
}

fn queue_app(s: &mut Scaffold, app_root: &Path, names: &Names, with_hello: bool, port: u16) {
    let r = Renderer::new(names).with("port", port.to_string());
    s.create(app_root.join("Cargo.toml"), r.render(workspace::APP_CARGO));
    s.create(app_root.join("src/lib.rs"), r.render(workspace::APP_LIB));
    s.create(app_root.join("src/main.rs"), r.render(workspace::APP_MAIN));

    let module_src = if with_hello {
        workspace::APP_MODULE_WITH_HELLO
    } else {
        workspace::APP_MODULE
    };
    s.create(app_root.join("src/module.rs"), r.render(module_src));

    if with_hello {
        s.create(app_root.join("tests/e2e.rs"), r.render(workspace::APP_E2E));
    }
}

fn queue_root_files(s: &mut Scaffold, base: &Path, names: &Names) {
    let r = Renderer::new(names);
    queue_env_files(s, base, names, "nestrs workspace", shared::ENV_WORKSPACE);
    s.create_if_missing(base.join("Justfile"), r.render(workspace::JUSTFILE));
    s.create_if_missing(base.join("test.just"), r.render(workspace::TEST_JUSTFILE));
    s.create_if_missing(base.join("db.just"), r.render(shared::DB_JUSTFILE));
    s.create_if_missing(base.join("compose.yml"), r.render(shared::COMPOSE));
    s.create_if_missing(base.join(".gitignore"), r.render(shared::GITIGNORE));
    s.create_if_missing(base.join(".dockerignore"), r.render(shared::DOCKERIGNORE));
}

fn print_root_next_steps(root: &Path) {
    println!();
    println!("Mode: workspace (crates/features/ + apps/*)");
    println!();
    println!("Next steps:");
    println!("  cd {}", root.display());
    println!("  nestrs run dev hello");
    println!("  Open http://localhost:3000/ in your browser");
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
