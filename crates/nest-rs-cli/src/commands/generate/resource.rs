//! `nestrs g resource <name>` — a DB-backed CRUD slice: an `#[expose]` entity,
//! a `CrudService`, and a `#[crud]` HTTP controller behind the app's guards.
//!
//! **Guards are not optional**: `Repo` filters every read by the ambient
//! `Ability` only an `AbilityGuard` installs, so the generator emits the guarded
//! shape and bootstraps `g auth` when the workspace has none.

use std::path::PathBuf;

use super::auth;
use super::cargo::{
    auth_deps, ensure_dev_deps, ensure_features_deps, ensure_workspace_deps, resource_deps,
    round_trip_dev_deps,
};
use super::migration::refuse_reserved;
use super::support::{Import, Wiring, data_layer, finish, wire_into_app, wiring_lines};
use crate::commands::resolve_start;
use crate::context::{Context, NestrsWorkspace, TargetApp};
use crate::error::{CliError, CliResult, read_if_exists};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_lines};
use crate::templates::resource;

pub(crate) struct ResourceOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    /// The app to wire into (`--app`).
    pub app: Option<String>,
    pub dry_run: bool,
}

pub(crate) fn run(opts: ResourceOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;
    let target = ctx.target_app(opts.app.as_deref())?;

    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);
    refuse_reserved(&names.snake, &names.table())?;
    let root = ws.feature_root(&names.snake);
    if root.exists() {
        return Err(CliError::FeatureExists {
            name: names.snake.clone(),
            path: root,
        });
    }

    let r = Renderer::new(&names);
    let mut s = Scaffold::new();

    // The guards the controller binds have to exist before it names them.
    let bootstrapped = if auth::exists(&ws) {
        None
    } else {
        Some(auth::queue(&mut s, &ws, Vec::new())?)
    };
    let scaffolded_auth = bootstrapped.is_some();

    s.create(root.join("entity.rs"), r.render(resource::ENTITY));
    s.create(root.join("service.rs"), r.render(resource::SERVICE));
    s.create(root.join("module.rs"), r.render(resource::MODULE));
    s.create(root.join("mod.rs"), r.render(resource::MOD));

    s.create(root.join("http/mod.rs"), r.render(resource::HTTP_MOD));
    s.create(root.join("http/module.rs"), r.render(resource::HTTP_MODULE));
    s.create(
        root.join("http/controller.rs"),
        r.render(resource::HTTP_CONTROLLER),
    );

    // Exactly one `edit` per path: a second re-reads the file from disk and
    // clobbers the first.
    let mut deps = resource_deps();
    let mut decls = Vec::new();
    if scaffolded_auth {
        deps.extend(auth_deps());
        decls.extend(auth::lib_decls());
    }
    decls.push(format!("pub mod {};", names.snake));
    // The app's round-trip test, beside the app the resource is wired into.
    let round_trip = round_trip_test(&ws, &target, &names)?;
    let mut workspace_deps = deps.clone();
    if round_trip.is_some() {
        workspace_deps.extend(round_trip_dev_deps());
    }
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(workspace_deps),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(deps));
    s.edit(ws.features_lib(), ensure_lines(decls));

    // The edge, the data layer every `#[crud]` route runs its unit of work on,
    // and the token route of the auth adapter this run wrote.
    let edge = Import::new(format!(
        "features::{}::{}",
        names.snake,
        names.http_module()
    ));
    let mut beside = data_layer().to_vec();
    if scaffolded_auth {
        beside.push(auth::app_import());
    }
    let imports: Vec<Import> = std::iter::once(edge.clone())
        .chain(beside.iter().cloned())
        .collect();
    let wiring = wire_into_app(&target, &mut s, &imports)?;
    if let Some(test) = &round_trip {
        test.queue(&mut s);
    }

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("resource `{}`", names.snake),
    )?;
    print_next_steps(
        &names,
        &wiring,
        &edge,
        &beside,
        round_trip.as_ref(),
        bootstrapped.as_ref(),
    );
    Ok(())
}

/// The round-trip test `g resource` writes into the app's `e2e` suite: the
/// test file, the suite root's module line and `boot` fixture, and the
/// suite's dev-dependencies.
struct RoundTrip {
    /// The app's crate root.
    app: PathBuf,
    test: PathBuf,
    test_body: String,
    main: PathBuf,
    /// The root's whole contents when it has none yet (`nestrs new`'s is
    /// empty), else the lines it gains.
    main_body: MainBody,
}

enum MainBody {
    Whole(String),
    Gains {
        module: String,
        boot: Option<String>,
    },
}

/// The test for a resource wired into `target`, when the workspace keeps the
/// migrator its fixture migrates with; none for a resource no app serves.
fn round_trip_test(
    ws: &NestrsWorkspace,
    target: &TargetApp,
    names: &Names,
) -> CliResult<Option<RoundTrip>> {
    let TargetApp::App { name, root } = target else {
        return Ok(None);
    };
    if !ws.migrations_lib().is_file() {
        return Ok(None);
    }
    let app = Names::parse(name);
    let r = Renderer::new(names)
        .with("app_snake", app.snake.clone())
        .with("app_module", app.module());
    let main = root.join("tests/e2e/main.rs");
    let module = format!("mod {};", names.snake);
    let main_body = match read_if_exists(&main)? {
        Some(current) if !current.trim().is_empty() => MainBody::Gains {
            module,
            boot: (!current.contains("fn boot(")).then(|| r.render(resource::E2E_BOOT)),
        },
        _ => MainBody::Whole(r.render(resource::E2E_MAIN)),
    };
    Ok(Some(RoundTrip {
        app: root.clone(),
        test: root.join(format!("tests/e2e/{}.rs", names.snake)),
        test_body: r.render(resource::E2E_TEST),
        main,
        main_body,
    }))
}

impl RoundTrip {
    fn queue(&self, s: &mut Scaffold) {
        s.create(&self.test, self.test_body.clone());
        match &self.main_body {
            MainBody::Whole(body) if self.main.is_file() => {
                let body = body.clone();
                s.edit(&self.main, Box::new(move |_| Some(body.clone())));
            }
            MainBody::Whole(body) => {
                s.create(&self.main, body.clone());
            }
            MainBody::Gains { module, boot } => {
                let lines = ensure_lines(vec![module.clone()]);
                let boot = boot.clone();
                s.edit(
                    &self.main,
                    Box::new(move |current: &str| {
                        let declared = lines(current).unwrap_or_else(|| current.to_owned());
                        let out = match &boot {
                            Some(boot) => format!("{declared}{boot}"),
                            None => declared,
                        };
                        (out != current).then_some(out)
                    }),
                );
            }
        }
        s.edit(
            self.app.join("Cargo.toml"),
            ensure_dev_deps(round_trip_dev_deps()),
        );
    }
}

fn print_next_steps(
    names: &Names,
    wiring: &Wiring,
    edge: &Import,
    beside: &[Import],
    round_trip: Option<&RoundTrip>,
    bootstrapped: Option<&auth::Secrets>,
) {
    let snake = &names.snake;
    println!();
    println!("Next steps:");
    println!("  1. Fill in `entity.rs` columns, then:  nestrs g migration create_{snake}");
    println!("  2. Grant the ability in `define`, in `crates/features/src/authz/ability.rs`, its");
    println!("     `_ab` parameter becoming `ab` — until you do, every route answers 403 and no");
    println!("     row crosses the data layer:");
    println!();
    println!("       ab.can(nest_rs::authz::Action::Manage, crate::{snake}::Entity);");
    println!();
    for (i, line) in wiring_lines(wiring, edge, beside).iter().enumerate() {
        let lead = if i == 0 { "  3. " } else { "     " };
        println!("{lead}{line}");
    }
    if let (Some(test), Wiring::Wired { root, .. }) = (round_trip, wiring) {
        let file = test
            .test
            .strip_prefix(root)
            .unwrap_or(&test.test)
            .display()
            .to_string();
        println!(
            "     Its round trip is `{file}` in that app: `nestrs run test e2e` passes it once"
        );
        println!("     the migration creates the table and the grant is in.");
    }
    println!("  4. Add transports:  nestrs g graphql|ws {}", names.kebab);
    if let Some(secrets) = bootstrapped {
        auth::print_bootstrapped(secrets);
    }
}
