//! Active runtime [`Environment`] — selects the `.env` cascade and branches
//! code paths.

use crate::source::real_env_var;

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
    /// DI graph (e.g. `OpenTelemetry::init`). Idempotent with `ConfigModule::for_root`.
    ///
    /// Parses the `.env` cascade into the in-crate map now (side-effect-free —
    /// no process-env mutation), so later `env_var` reads see dotenv values and
    /// the one-time file-read cost is paid at startup rather than mid-request.
    pub fn init() -> Self {
        let env = Self::from_env();
        let _ = crate::dotenv::dotenv_values();
        env
    }

    pub fn from_env() -> Self {
        // `NESTRS_ENV` selects the cascade, so it must come from the real
        // process env, never a `.env` file — read it without the dotenv
        // fallback (which would also recurse through `dotenv_values`).
        match real_env_var("NESTRS_ENV").as_deref().map(str::trim) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_is_lowercase_for_each_variant() {
        assert_eq!(Environment::Development.as_str(), "development");
        assert_eq!(Environment::Test.as_str(), "test");
        assert_eq!(Environment::Staging.as_str(), "staging");
        assert_eq!(Environment::Production.as_str(), "production");
    }

    #[test]
    fn is_production_matches_only_production() {
        assert!(Environment::Production.is_production());
        assert!(!Environment::Development.is_production());
        assert!(!Environment::Test.is_production());
        assert!(!Environment::Staging.is_production());
    }

    #[test]
    fn default_is_development() {
        assert_eq!(Environment::default(), Environment::Development);
    }
}
