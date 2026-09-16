//! Where am I? — structure-based context detection.
//!
//! There is no config file: the directory tree *is* the configuration. A
//! single [`Context::detect`] resolves the workspace and whether the cursor
//! sits inside an app, which the generators use to decide what to auto-wire.

mod detect;
mod workspace;

pub(crate) use detect::Context;
pub(crate) use workspace::{
    DEFAULT_ENV_PREFIX, ENV_PREFIX_VAR, EnvPrefixSource, NestrsWorkspace, env_prefix,
    framework_pin, validate_env_prefix, var_name,
};
