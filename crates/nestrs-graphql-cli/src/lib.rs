//! Schema tooling shared by every nestrs GraphQL app.
//!
//! A GraphQL app commits its schema as SDL so the API surface is reviewable in
//! diffs. Because the schema is composed from the resolvers *linked into a
//! binary* (they self-register at link time), it can only be rendered from
//! inside the app — so each app's binary exposes a `schema` subcommand that
//! calls [`run`]. With multiple apps (and federation, where each app is a
//! subgraph), that logic is identical everywhere; this crate holds it once.
//!
//! The crate is deliberately minimal — emit and drift-check. Federation-aware
//! commands (subgraph SDL, composition) land here when federation itself does.

use std::path::Path;
use std::process::ExitCode;

use nestrs_core::{App, Module};
use nestrs_graphql::schema_sdl;

/// Emit or drift-check an app's GraphQL SDL, driving its `schema` subcommand.
///
/// `args` are the tokens *after* the subcommand:
/// - none — render the schema and write it to `default_path` (the app's
///   committed `schema.graphql`).
/// - `--check` — render in memory and compare against the committed file,
///   returning a failure code on drift. This is the CI guard.
/// - a non-flag token overrides `default_path`.
///
/// `M` is the app's root module. The schema is built from the resolvers linked
/// into *this* binary, which is why the call must live in the app's own binary;
/// [`App::context`] builds the container without starting a transport.
pub fn run<M: Module>(default_path: &str, args: impl IntoIterator<Item = String>) -> ExitCode {
    let mut check = false;
    let mut path_override: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            other if !other.starts_with('-') => path_override = Some(other.to_owned()),
            other => {
                eprintln!("unknown argument `{other}` (expected `--check` or a path)");
                return ExitCode::FAILURE;
            }
        }
    }
    let path = Path::new(path_override.as_deref().unwrap_or(default_path));

    let sdl = schema_sdl(&App::context::<M>());

    if check {
        match std::fs::read_to_string(path) {
            Ok(committed) if committed == sdl => {
                println!("{} is up to date", path.display());
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!(
                    "{} is out of date — run `just graphql-schema` and commit the result",
                    path.display()
                );
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!(
                    "cannot read {}: {err} — run `just graphql-schema`",
                    path.display()
                );
                ExitCode::FAILURE
            }
        }
    } else {
        match std::fs::write(path, &sdl) {
            Ok(()) => {
                println!("wrote {}", path.display());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("cannot write {}: {err}", path.display());
                ExitCode::FAILURE
            }
        }
    }
}
