//! The `nestrs` command, as a library.
//!
//! The binary is the product; this target exists so that one definition of a
//! rule serves two callers. `nestrs lint` runs the naming rules over a
//! developer's tree and `nest-rs-conformance` runs **the same code** over the
//! framework's own, because a rule the framework ships and does not itself pass
//! is the failure that matters, and a second implementation in the suite is how
//! the two come to disagree without anyone noticing.
//!
//! The same holds for a mirror: `nestrs doctor` answers what an app makes of a
//! variable and of the `.env` cascade without linking the loader, and the suite
//! runs [`resolve_variable`] and [`cascade_refusals`] beside the loader they
//! mirror, so the two cannot drift apart unseen.
//!
//! Nothing here is an install surface: `nestrs` is reached with
//! `cargo install --locked nest-rs-cli`, never with `cargo add`. So the seam is
//! only what a second caller needs — [`lint`], [`reserved_words`],
//! [`resolve_variable`], [`cascade_refusals`] and [`scaffolded_var`]; the rest
//! is the binary's own and hidden from the docs.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool's output is its interface; tracing is for the apps it scaffolds"
)]

pub mod lint;

pub use commands::doctor::{Resolution, cascade_refusals, resolve_variable};
pub use naming::reserved_words;

/// The variable the prefix is read from, and the one name no prefix renames —
/// spelled once for this crate, in `context`, and read from there.
pub const ENV_PREFIX_VAR: &str = context::ENV_PREFIX_VAR;

/// A framework variable's full name as this CLI writes it into a project —
/// `<PREFIX>_<NAMESPACE>__<KEY>`, under the prefix the environment names, or
/// the default when it names none or an unusable one.
///
/// The CLI's own suite asserts what a scaffold wrote with it, so a name the
/// suite expects and a name the CLI writes are one derivation rather than a
/// mirror of it: a literal `NESTRS_AUTHN__SECRET` in a test fails the moment
/// the suite runs under `NESTRS_ENV_PREFIX=ACME`, the run that proves a rename
/// reaches everything.
pub fn scaffolded_var(namespace: &str, key: &str) -> String {
    context::var_name(&context::env_prefix(), &namespace.to_uppercase(), key)
}

// The binary's own entry points. `pub` because `main.rs` is a separate target,
// `#[doc(hidden)]` because they are not API: `nestrs` is a command, and the
// clap surface behind it moves whenever the command surface does.
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod error;

mod commands;
mod context;
mod naming;
mod port;
mod scaffold;
mod templates;
mod version;
