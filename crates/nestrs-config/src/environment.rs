//! Active runtime [`Environment`] — selects the `.env` cascade and branches
//! code paths.

use crate::loader::env_var;

/// Read from the reserved `NESTRS_ENV`. This is the one framework variable
/// **outside** the `NESTRS_<DOMAIN>__<KEY>` scheme — it selects which `.env`
/// files to load, so it must come from the real process environment, not a
/// `.env` file. Unset or unrecognised ⇒ [`Development`](Self::Development).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Environment {
    #[default]
    Development,
    /// `.env.local` is **not** loaded so tests stay hermetic.
    Test,
    Staging,
    Production,
}

impl Environment {
    /// Call at the top of `main` before anything that reads the env outside the
    /// DI graph (e.g. `Telemetry::init`). Idempotent with `ConfigModule::for_root`.
    pub fn init() -> Self {
        crate::dotenv::ensure_env_loaded();
        Self::from_env()
    }

    pub fn from_env() -> Self {
        match env_var("NESTRS_ENV").as_deref().map(str::trim) {
            Some("production" | "prod") => Self::Production,
            Some("staging" | "stage") => Self::Staging,
            Some("test") => Self::Test,
            _ => Self::Development,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    pub fn is_production(&self) -> bool {
        matches!(self, Self::Production)
    }
}
