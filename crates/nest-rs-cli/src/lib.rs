//! The `nestrs` command, as a library.
//!
//! The binary is the product; this target exists so this crate's own suites
//! call what the commands run — [`resolve_variable`] and [`cascade_refusals`]
//! beside the loader they mirror. Nothing here is an install surface: the seam
//! is only what a second caller needs, and the rest is hidden from the docs.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool's output is its interface; tracing is for the apps it scaffolds"
)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

pub mod lint;

pub use commands::doctor::{Resolution, cascade_refusals, resolve_variable};

/// The variable the prefix is read from, and the one name no prefix renames.
pub const ENV_PREFIX_VAR: &str = context::ENV_PREFIX_VAR;

/// A framework variable's full name as this CLI writes it into a project —
/// `<PREFIX>_<NAMESPACE>__<KEY>`, under the prefix the environment names, or
/// the default when it names none or an unusable one.
///
/// The CLI's own suite asserts what a scaffold wrote with it, so a test never
/// spells a literal name that breaks under `NESTRS_ENV_PREFIX=ACME`.
pub fn scaffolded_var(namespace: &str, key: &str) -> String {
    context::var_name(&context::env_prefix(), &namespace.to_uppercase(), key)
}

// `pub` because `main.rs` is a separate target; `#[doc(hidden)]` because the
// clap surface is not API.
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
