//! Active runtime [`Environment`] — selects the `.env` cascade and branches
//! code paths.

use nest_rs_core::EnvPrefix;

use crate::source::real_env_var;

/// Read from the reserved `<PREFIX>_ENV`, outside the `<PREFIX>_<DOMAIN>__<KEY>`
/// scheme: it selects which `.env` files to load, so it comes from the real
/// process environment alone. Unset or unrecognised ⇒
/// [`Development`](Self::Development).
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
    /// Parse the `.env` cascade and merge it into the **process environment**
    /// (set-if-absent), for every consumer that reads `std::env::var` —
    /// `<PREFIX>_LOG*`, `OpenTelemetry::init`, a `migrate` binary,
    /// [`Environment::declared`]. Call it at the top of `main`; calling it twice
    /// is harmless.
    ///
    /// # Threading
    ///
    /// The merge writes through `std::env::set_var`, which is unsound only when
    /// it races a concurrent `getenv` on another thread. Calling this at the top
    /// of `main` — before the runtime spawns anything — discharges that
    /// obligation; calling it from a spawned task does not.
    pub fn init() -> Self {
        let env = Self::from_env();
        crate::dotenv::publish_dotenv_values();
        env
    }

    /// Read the active environment from `<PREFIX>_ENV` (real process env only).
    #[expect(
        clippy::print_stderr,
        reason = "this runs at the top of main, before any subscriber exists; stderr is the one sink guaranteed visible"
    )]
    pub fn from_env() -> Self {
        let var = Self::var_name();
        let raw = real_env_var(&var);
        let (env, unrecognized) = classify(raw.as_deref());
        if let Some(value) = unrecognized {
            eprintln!(
                "nestrs: WARNING — unrecognized {var}={value:?}; falling back to \
                 `development`. A misspelled production value loads the development `.env` \
                 cascade in production. Use one of: development, test, staging, production."
            );
        }
        env
    }

    /// The environment the process **declares**, or `None` when nobody set it.
    ///
    /// Unset, empty and unrecognised are all `None`, so a development-only
    /// affordance gated on this fails closed — unlike [`from_env`](Self::from_env),
    /// which maps absence to [`Development`](Self::Development).
    pub fn declared() -> Option<Self> {
        declare(real_env_var(&Self::var_name()).as_deref())
    }

    /// The variable this reads — `<PREFIX>_ENV`.
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

/// Classify a raw `<PREFIX>_ENV` value into an [`Environment`], returning
/// `Some(value)` in the second slot when the value was **set but
/// unrecognized** (so the caller can surface it) and `None` when it was unset,
/// empty, or an explicit development alias.
fn classify(raw: Option<&str>) -> (Environment, Option<String>) {
    match raw.map(str::trim) {
        Some("production" | "prod") => (Environment::Production, None),
        Some("staging" | "stage") => (Environment::Staging, None),
        Some("test") => (Environment::Test, None),
        Some("development" | "dev" | "") | None => (Environment::Development, None),
        Some(other) => (Environment::Development, Some(other.to_owned())),
    }
}

/// [`Environment::declared`]'s pure half: a positively declared value maps
/// through [`classify`], everything else is `None`.
fn declare(raw: Option<&str>) -> Option<Environment> {
    let value = raw.map(str::trim).filter(|v| !v.is_empty())?;
    match classify(Some(value)) {
        (env, None) => Some(env),
        (_, Some(_)) => None,
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
    fn declare_answers_none_unless_positively_declared() {
        assert_eq!(declare(None), None);
        assert_eq!(declare(Some("")), None);
        assert_eq!(declare(Some("   ")), None);
        assert_eq!(declare(Some("producton")), None);
        assert_eq!(declare(Some("development")), Some(Environment::Development));
        assert_eq!(
            declare(Some(" development ")),
            Some(Environment::Development)
        );
        assert_eq!(declare(Some("dev")), Some(Environment::Development));
        assert_eq!(declare(Some("test")), Some(Environment::Test));
        assert_eq!(declare(Some("production")), Some(Environment::Production));
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts what init wrote into the process environment"
    )]
    fn init_publishes_the_cascade_into_the_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_INIT_A=base\nCASCADE_INIT_B=base")?;
            jail.create_file(".env.development", "CASCADE_INIT_B=dev")?;
            jail.set_env(Environment::var_name(), "development");
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
        let (env, unrecognized) = classify(Some("producton"));
        assert_eq!(env, Environment::Development);
        assert_eq!(unrecognized.as_deref(), Some("producton"));
    }
}
