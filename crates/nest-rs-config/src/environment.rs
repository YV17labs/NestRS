//! Active runtime [`Environment`] — selects the `.env` cascade and branches
//! code paths.

use crate::source::real_env_var;

/// Read from the reserved `NESTRS_ENV`. This is the one framework variable
/// **outside** the `NESTRS_<DOMAIN>__<KEY>` scheme — it selects which `.env`
/// files to load, so it must come from the real process environment, not a
/// `.env` file. Unset or unrecognised ⇒ [`Development`](Self::Development).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Environment {
    /// Local development — the default when `NESTRS_ENV` is unset or unrecognised.
    #[default]
    Development,
    /// `.env.local` is **not** loaded so tests stay hermetic.
    Test,
    /// Pre-production staging.
    Staging,
    /// Production.
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

    /// Read the active environment from `NESTRS_ENV` (real process env only).
    pub fn from_env() -> Self {
        // `NESTRS_ENV` selects the cascade, so it must come from the real
        // process env, never a `.env` file — read it without the dotenv
        // fallback (which would also recurse through `dotenv_values`).
        let raw = real_env_var("NESTRS_ENV");
        let (env, unrecognized) = classify(raw.as_deref());
        // Set but UNRECOGNIZED (a typo like `producton`) must not silently load
        // the dev cascade in production (CONF-I4). This runs at the top of
        // `main`, before any tracing subscriber exists, so surface it on stderr
        // where it is guaranteed visible rather than as a dropped log.
        if let Some(value) = unrecognized {
            eprintln!(
                "nestrs: WARNING — unrecognized NESTRS_ENV={value:?}; falling back to \
                 `development`. A misspelled production value loads the development `.env` \
                 cascade in production. Use one of: development, test, staging, production."
            );
        }
        env
    }

    /// The lowercase name of this environment (`"development"`, `"production"`, …).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    /// Whether this is [`Production`](Self::Production).
    pub fn is_production(&self) -> bool {
        matches!(self, Self::Production)
    }
}

/// Classify a raw `NESTRS_ENV` value into an [`Environment`], returning
/// `Some(value)` in the second slot when the value was **set but
/// unrecognized** (so the caller can surface it) and `None` when it was unset,
/// empty, or an explicit development alias. Pure, so it is testable without
/// mutating the process environment.
fn classify(raw: Option<&str>) -> (Environment, Option<String>) {
    match raw.map(str::trim) {
        Some("production" | "prod") => (Environment::Production, None),
        Some("staging" | "stage") => (Environment::Staging, None),
        Some("test") => (Environment::Test, None),
        Some("development" | "dev" | "") | None => (Environment::Development, None),
        Some(other) => (Environment::Development, Some(other.to_owned())),
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

    #[test]
    fn classify_recognizes_each_environment_and_its_aliases() {
        assert_eq!(classify(Some("production")).0, Environment::Production);
        assert_eq!(classify(Some("prod")).0, Environment::Production);
        assert_eq!(classify(Some("staging")).0, Environment::Staging);
        assert_eq!(classify(Some("stage")).0, Environment::Staging);
        assert_eq!(classify(Some("test")).0, Environment::Test);
        assert_eq!(classify(Some(" production ")).0, Environment::Production); // trimmed
    }

    #[test]
    fn classify_treats_unset_empty_and_dev_aliases_as_silent_development() {
        for raw in [None, Some(""), Some("  "), Some("development"), Some("dev")] {
            let (env, unrecognized) = classify(raw);
            assert_eq!(env, Environment::Development, "for {raw:?}");
            assert!(unrecognized.is_none(), "must be silent for {raw:?}");
        }
    }

    #[test]
    fn classify_flags_a_set_but_unrecognized_value_while_defaulting_to_development() {
        // CONF-I4: a typo like `producton` must fall back to development *and*
        // report the offending value so the caller can warn, never silently
        // load the dev cascade in production.
        let (env, unrecognized) = classify(Some("producton"));
        assert_eq!(env, Environment::Development);
        assert_eq!(unrecognized.as_deref(), Some("producton"));
    }
}
