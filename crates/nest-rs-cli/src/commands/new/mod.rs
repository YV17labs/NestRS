//! `nestrs new <name>` — infer the layout from the tree and scaffold it:
//! a fresh monorepo, or an app inside an existing workspace. Both commit
//! through a transactional `Scaffold`.

mod command;
mod workspace;

pub use command::{NewOptions, run, run_cargo_check};

pub(crate) use command::{prefix_vars, queue_agent_files, queue_env_files};
