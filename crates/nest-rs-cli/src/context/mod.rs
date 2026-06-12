//! Where am I? — structure-based context detection.
//!
//! There is no config file: the directory tree *is* the configuration. A
//! single [`Context::detect`] resolves the workspace and whether the cursor
//! sits inside an app, which the generators use to decide what to auto-wire.

mod context;
mod workspace;

pub use context::Context;
pub use workspace::NestrsWorkspace;
