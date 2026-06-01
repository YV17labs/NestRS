//! The active runtime [`Environment`] — the profile that selects which `.env`
//! files load and that application code can branch on.

use crate::loader::env_var;

/// The deployment environment, read from the reserved `NESTRS_ENV` variable.
///
/// `NESTRS_ENV` is the one framework variable that sits **outside** the
/// `NESTRS_<DOMAIN>__<KEY>` scheme: it is the bootstrap selector read before any
/// config loads (it decides which `.env` files are layered, see
/// [`crate::ConfigModule::for_root`]). It must come from the real process
/// environment — a value a `.env` file sets cannot select which `.env` files to
/// load. Unset (or unrecognised) means [`Development`](Self::Development).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Environment {
    /// Local development — the default. Loads `.env.local` overrides.
    #[default]
    Development,
    /// Automated tests. `.env.local` is **not** loaded (tests must be hermetic).
    Test,
    /// Pre-production.
    Staging,
    /// Production.
    Production,
}

impl Environment {
    /// Read `NESTRS_ENV` from the real process environment. Accepts the long or
    /// short spellings (`production`/`prod`, `development`/`dev`); anything
    /// unrecognised falls back to [`Development`](Self::Development).
    pub fn from_env() -> Self {
        match env_var("NESTRS_ENV").as_deref().map(str::trim) {
            Some("production" | "prod") => Self::Production,
            Some("staging" | "stage") => Self::Staging,
            Some("test") => Self::Test,
            _ => Self::Development,
        }
    }

    /// The lowercase name used both as the `deployment.environment` attribute and
    /// as the `<env>` segment of the `.env.<env>` / `.env.<env>.local` files.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    /// Whether this is the production environment — the usual gate for stricter
    /// defaults (sampling, log format, …).
    pub fn is_production(&self) -> bool {
        matches!(self, Self::Production)
    }
}
