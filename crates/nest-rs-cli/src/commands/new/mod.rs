//! `nestrs new <name>` — infer the layout from the tree and scaffold it:
//! a fresh monorepo, an app inside an existing workspace, or a single crate
//! (`--standalone`). All three commit through a transactional `Scaffold`.

mod command;
mod standalone;
mod workspace;

pub use command::{NewOptions, NewTemplate, project_dir_for_check, run, run_cargo_check};

pub(crate) use command::queue_env_files;
