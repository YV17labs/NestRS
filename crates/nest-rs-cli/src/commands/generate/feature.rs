use std::path::PathBuf;

use crate::context::NestrsWorkspace;
use crate::error::{CliError, CliResult};
use crate::fs::{append_feature_mod, render, render_with_extra, write_file};
use crate::naming::Names;
use crate::templates::feature;

pub struct FeatureOptions {
    pub name: String,
    pub path: Option<PathBuf>,
    pub http: bool,
}

pub fn run(opts: FeatureOptions) -> CliResult<()> {
    let start = opts
        .path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let ws = NestrsWorkspace::require(&start)?;
    let names = Names::parse(&opts.name);
    let feature_root = ws.features_root().join(&names.snake);

    if feature_root.exists() {
        return Err(CliError::FeatureExists {
            name: names.snake.clone(),
            path: feature_root,
        });
    }

    let http_mod_line = if opts.http {
        "pub mod http;"
    } else {
        ""
    };
    let http_pub_line = if opts.http {
        format!(
            "pub use http::{{{controller}, {http_module}}};",
            controller = names.controller(),
            http_module = names.http_module(),
        )
    } else {
        String::new()
    };

    write_file(
        &feature_root.join("mod.rs"),
        &render_with_extra(
            feature::FEATURE_MOD,
            &names,
            &[
                ("http_mod_line", http_mod_line),
                ("http_pub_line", &http_pub_line),
            ],
        ),
    )?;
    write_file(
        &feature_root.join("module.rs"),
        &render(feature::FEATURE_MODULE, &names),
    )?;
    write_file(
        &feature_root.join("service.rs"),
        &render(feature::FEATURE_SERVICE, &names),
    )?;

    if opts.http {
        write_file(
            &feature_root.join("http/mod.rs"),
            &render(feature::FEATURE_HTTP_MOD, &names),
        )?;
        write_file(
            &feature_root.join("http/module.rs"),
            &render(feature::FEATURE_HTTP_MODULE, &names),
        )?;
        write_file(
            &feature_root.join("http/controller.rs"),
            &render(feature::FEATURE_HTTP_CONTROLLER, &names),
        )?;
    }

    append_feature_mod(&ws.features_lib(), &names.snake)?;

    println!(
        "Created feature `{}` at {}",
        names.snake,
        feature_root.display()
    );
    print_next_steps(&names, opts.http);
    Ok(())
}

fn print_next_steps(names: &Names, http: bool) {
    println!();
    println!("Next steps:");
    if http {
        println!(
            "  1. Import `features::{}::{}` in your app root `module.rs`",
            names.snake, names.http_module()
        );
        println!("  2. cargo check -p <your-app>");
    } else {
        println!(
            "  1. Add an adapter: nestrs g feature {} --http",
            names.kebab
        );
        println!("     (or copy `users/http/` from the reference feature)");
    }
    println!(
        "  Reference: crates/features/src/users/ — copy before inventing a second pattern."
    );
}
