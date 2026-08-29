//! One module per command the CLI dispatches to.
//!
//! `version` and `about` are the exception and live in [`crate::cli`]: both are
//! four `println!`s over compile-time metadata, and `about`'s tagline is shared
//! with clap's own `about =` attribute.

use std::path::PathBuf;

mod doctor;
mod generate;
mod info;
mod lint;
mod new;
mod run;
mod toolchain;
mod update;

/// Where a command reads the tree from: the explicit `-p` / `--path`, or the
/// directory the developer stands in.
///
/// One spelling for every command that takes that flag — the fallback is the
/// same decision each time, and four copies of it are four places a future
/// `NESTRS_PROJECT` or a friendlier failure would have to be written.
pub(crate) fn resolve_start(path: Option<PathBuf>) -> PathBuf {
    path.unwrap_or_else(|| std::env::current_dir().expect("cwd"))
}

pub use doctor::{DoctorOptions, run as run_doctor};
pub(crate) use generate::queue_db_crates;
pub use generate::{
    AdapterOptions, AuthOptions, EntityOptions, FeatureOptions, MigrationOptions, ResourceOptions,
    run_adapter, run_auth, run_entity, run_feature, run_migration, run_resource,
};
pub use info::{InfoOptions, run as run_info};
pub use lint::{LintOptions, run as run_lint};
pub use new::{NewOptions, run as run_new, run_cargo_check};
// The prefix-dependent renderer keys, seeded by `Renderer::new` and re-seeded by
// `--env-prefix` from the same list.
pub(crate) use new::prefix_vars;
pub use run::{RunOptions, run as run_task};
pub use update::{UpdateOptions, run as run_update};
