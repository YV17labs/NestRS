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
//! variable without linking the loader, and the suite runs
//! [`resolve_variable`] beside the loader it mirrors, so the two cannot drift
//! apart unseen.
//!
//! Nothing here is an install surface: `nestrs` is reached with
//! `cargo install --locked nest-rs-cli`, never with `cargo add`. So the seam is
//! only what a second caller needs — [`lint`], [`reserved_words`] and
//! [`resolve_variable`]; the rest is the binary's own and hidden from the docs.

pub mod lint;

pub use commands::doctor::{Resolution, resolve_variable};
pub use naming::reserved_words;

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
