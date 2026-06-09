//! `nestrs g feature <name>` — a transport-agnostic port under
//! `crates/features/src/<name>/` (mod + module + service). Add transports
//! afterwards with `g http|graphql|ws|queue|schedule|mcp <name>`.

use std::path::PathBuf;

use super::{finish, resolve_start};
use crate::context::Context;
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold, ensure_decl};
use crate::templates::feature;

pub struct FeatureOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub fn run(opts: FeatureOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.ok_or(CliError::NotNestrsWorkspace)?;

    let names = Names::parse(&opts.name);
    let root = ws.feature_root(&names.snake);
    if root.exists() {
        return Err(CliError::FeatureExists {
            name: names.snake.clone(),
            path: root,
        });
    }

    let r = Renderer::new(&names);
    let mut s = Scaffold::new();
    s.create(root.join("mod.rs"), r.render(feature::MOD));
    s.create(root.join("module.rs"), r.render(feature::MODULE));
    s.create(root.join("service.rs"), r.render(feature::SERVICE));
    s.edit(
        ws.features_lib(),
        ensure_decl(&format!("pub mod {};", names.snake)),
    );

    finish(
        s,
        opts.dry_run,
        &ws.root,
        &format!("Created feature `{}`", names.snake),
    )?;
    print_next_steps(&names);
    Ok(())
}

fn print_next_steps(names: &Names) {
    println!();
    println!("Next steps:");
    println!("  Add a transport:  nestrs g http {}", names.kebab);
    println!(
        "                    nestrs g graphql|ws|queue|schedule|mcp {}",
        names.kebab
    );
    println!("  DB-backed CRUD?   nestrs g resource {}", names.kebab);
    println!("  Reference:        crates/features/src/users/");
}
