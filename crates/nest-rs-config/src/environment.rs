//! Active runtime [`Environment`] — selects the `.env` cascade and branches
//! code paths.

use nest_rs_core::EnvPrefix;

use crate::source::real_env_var;

/// Read from the reserved `<PREFIX>_ENV` (`NESTRS_ENV` by default). This is the
/// one framework variable **outside** the `<PREFIX>_<DOMAIN>__<KEY>` scheme —
/// it selects which `.env` files to load, so it must come from the real process
/// environment, not a `.env` file.
/// Unset or unrecognised ⇒ [`Development`](Self::Development).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Environment {
    /// Local development — the default when `<PREFIX>_ENV` is unset or unrecognised.
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
    /// Call at the top of `main`, before anything that reads the environment
    /// outside the DI graph. Idempotent with `ConfigModule::for_root`.
    ///
    /// Two effects, both one-shot:
    ///
    /// 1. the `.env` cascade is parsed into the in-crate map, so later
    ///    `env_var` reads see dotenv values without paying the file read
    ///    mid-request;
    /// 2. the same values are merged into the **process environment**
    ///    (set-if-absent — the real env always wins), so the many consumers
    ///    that only know `std::env::var` behave as the cascade says: the
    ///    framework's own `NESTRS_LOG*` logging setup, `OpenTelemetry::init`,
    ///    and any `migrate`/`seed` binary of yours.
    ///
    /// # Threading
    ///
    /// The merge writes through `std::env::set_var`, which is unsound only when
    /// it races a concurrent `getenv` on another thread. Calling this at the top
    /// of `main` — before the runtime spawns anything — discharges that
    /// obligation; calling it from a spawned task does not.
    pub fn init() -> Self {
        let env = Self::from_env();
        // Parses the cascade once and publishes that same map — the one
        // process-env write a running app makes, and the reason `init` belongs
        // at the top of `main`: its soundness obligation is discharged by being
        // single-threaded here, nowhere else.
        crate::dotenv::publish_dotenv_values();
        env
    }

    /// Read the active environment from `<PREFIX>_ENV` (real process env only).
    pub fn from_env() -> Self {
        // `<PREFIX>_ENV` selects the cascade, so it must come from the real
        // process env, never a `.env` file — read it without the dotenv
        // fallback (which would also recurse through `dotenv_values`).
        let var = Self::var_name();
        let raw = real_env_var(&var);
        let (env, unrecognized) = classify(raw.as_deref());
        // Set but UNRECOGNIZED (a typo like `producton`) must not silently load
        // the dev cascade in production (CONF-I4). This runs at the top of
        // `main`, before any tracing subscriber exists, so surface it on stderr
        // where it is guaranteed visible rather than as a dropped log.
        if let Some(value) = unrecognized {
            eprintln!(
                "nestrs: WARNING — unrecognized {var}={value:?}; falling back to \
                 `development`. A misspelled production value loads the development `.env` \
                 cascade in production. Use one of: development, test, staging, production."
            );
        }
        env
    }

    /// The variable this reads — `NESTRS_ENV`, or `<PREFIX>_ENV` under
    /// [`env_prefix!`](nest_rs_core::env_prefix!). Public because a harness that
    /// must decide the environment before the framework does (`nest-rs-testing`)
    /// has to name the same variable, and a second literal there is exactly how
    /// a rename half-lands.
    pub fn var_name() -> String {
        EnvPrefix::var("ENV")
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

    // The merge is what makes a raw `std::env::var` reader — the framework's own
    // `NESTRS_LOG*` setup, a `migrate` binary — see the cascade at all, and it
    // is the whole reason `init` belongs at the top of `main`. Every other test
    // in this crate would still pass if that call vanished.
    #[test]
    #[allow(clippy::result_large_err)]
    fn init_publishes_the_cascade_into_the_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_INIT_A=base\nCASCADE_INIT_B=base")?;
            jail.create_file(".env.development", "CASCADE_INIT_B=dev")?;
            jail.set_env("NESTRS_ENV", "development");
            jail.set_env("CASCADE_INIT_C", "from_real_env");
            jail.create_file(".env.development.local", "CASCADE_INIT_C=from_file")?;

            assert_eq!(Environment::init(), Environment::Development);

            assert_eq!(std::env::var("CASCADE_INIT_A").unwrap(), "base");
            assert_eq!(
                std::env::var("CASCADE_INIT_B").unwrap(),
                "dev",
                "the cascade's precedence carries into the process env",
            );
            assert_eq!(
                std::env::var("CASCADE_INIT_C").unwrap(),
                "from_real_env",
                "set-if-absent: the real environment still wins",
            );
            Ok(())
        });
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
