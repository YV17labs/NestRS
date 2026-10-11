//! Active runtime [`Environment`] — selects the `.env` cascade and branches
//! code paths.

use std::sync::Once;

use nest_rs_core::EnvPrefix;

use crate::source::real_env_var;

/// Read from the reserved `<PREFIX>_ENV`, outside the `<PREFIX>_<DOMAIN>__<KEY>`
/// scheme: it selects which `.env` files to load, so it comes from the real
/// process environment alone. Unset, empty or unrecognised ⇒
/// [`Production`](Self::Production): a development convenience is armed only
/// by a process that declares its profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Environment {
    /// Local development, declared as `development` or `dev`.
    Development,
    /// Declared as `test`; `.env.local` is **not** loaded so tests stay hermetic.
    Test,
    /// Pre-production staging, declared as `staging` or `stage`.
    Staging,
    /// Production, declared as `production` or `prod` — and what a process that
    /// leaves `<PREFIX>_ENV` unset, empty or unrecognised runs as.
    #[default]
    Production,
}

impl Environment {
    /// Read the active environment from `<PREFIX>_ENV` (real process env only).
    ///
    /// A value that names no environment is read as
    /// [`Production`](Self::Production), and said once per process on stderr,
    /// by the variable's name alone.
    #[expect(
        clippy::print_stderr,
        reason = "this runs at the top of main, before any subscriber exists; stderr is the one sink guaranteed visible"
    )]
    pub fn from_env() -> Self {
        static WARNED: Once = Once::new();
        let var = Self::var_name();
        let (env, unrecognized) = classify(real_env_var(&var).as_deref());
        if unrecognized {
            WARNED.call_once(|| {
                eprintln!(
                    "nestrs: WARNING — {var} holds a value that is none of development, dev, \
                     test, staging, stage, production, prod; it is read as `production`."
                );
            });
        }
        env
    }

    /// The environment the process **declares**, or `None` when nobody set it.
    ///
    /// Unset, empty and unrecognised are all `None`, where
    /// [`from_env`](Self::from_env) reads them as
    /// [`Production`](Self::Production).
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

    /// Whether this is a deployment — [`Staging`](Self::Staging) or
    /// [`Production`](Self::Production) — where a config's
    /// [`defaults`](crate::Config::defaults) arms no development convenience
    /// and stays its `Default`.
    pub fn is_deployed(&self) -> bool {
        match self {
            Self::Staging | Self::Production => true,
            Self::Development | Self::Test => false,
        }
    }
}

/// Classify a raw `<PREFIX>_ENV` value into an [`Environment`], with `true`
/// in the second slot when the value was **set but unrecognized** (so the
/// caller can surface it) — never for unset or empty, which are production
/// silently.
fn classify(raw: Option<&str>) -> (Environment, bool) {
    match raw.map(str::trim) {
        Some("development" | "dev") => (Environment::Development, false),
        Some("test") => (Environment::Test, false),
        Some("staging" | "stage") => (Environment::Staging, false),
        Some("production" | "prod" | "") | None => (Environment::Production, false),
        Some(_) => (Environment::Production, true),
    }
}

/// [`Environment::declared`]'s pure half: a positively declared value maps
/// through [`classify`], everything else is `None`.
fn declare(raw: Option<&str>) -> Option<Environment> {
    let value = raw.map(str::trim).filter(|v| !v.is_empty())?;
    match classify(Some(value)) {
        (env, false) => Some(env),
        (_, true) => None,
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
    fn is_deployed_matches_staging_and_production_alone() {
        assert!(Environment::Production.is_deployed());
        assert!(Environment::Staging.is_deployed());
        assert!(!Environment::Development.is_deployed());
        assert!(!Environment::Test.is_deployed());
    }

    #[test]
    fn default_is_production() {
        assert_eq!(Environment::default(), Environment::Production);
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
    fn classify_recognizes_each_environment_and_its_aliases() {
        assert_eq!(classify(Some("production")).0, Environment::Production);
        assert_eq!(classify(Some("prod")).0, Environment::Production);
        assert_eq!(classify(Some("staging")).0, Environment::Staging);
        assert_eq!(classify(Some("stage")).0, Environment::Staging);
        assert_eq!(classify(Some("test")).0, Environment::Test);
        assert_eq!(classify(Some(" production ")).0, Environment::Production); // trimmed
    }

    #[test]
    fn classify_reads_unset_and_empty_as_silent_production() {
        for raw in [None, Some(""), Some("  ")] {
            let (env, unrecognized) = classify(raw);
            assert_eq!(env, Environment::Production, "for {raw:?}");
            assert!(!unrecognized, "must be silent for {raw:?}");
        }
    }

    #[test]
    fn classify_reads_the_development_aliases_as_silent_development() {
        for raw in [Some("development"), Some("dev"), Some(" dev ")] {
            let (env, unrecognized) = classify(raw);
            assert_eq!(env, Environment::Development, "for {raw:?}");
            assert!(!unrecognized, "must be silent for {raw:?}");
        }
    }

    #[test]
    fn classify_flags_a_set_but_unrecognized_value_while_reading_it_as_production() {
        let (env, unrecognized) = classify(Some("prd"));
        assert_eq!(env, Environment::Production);
        assert!(unrecognized, "the value is reported as unrecognised");
    }
}
