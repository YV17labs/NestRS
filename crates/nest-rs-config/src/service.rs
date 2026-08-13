//! Framework env-var scheme `<PREFIX>_<DOMAIN>__<KEY>` and the typed
//! [`ConfigService`] reader handed to a config's `from_env`.
//!
//! `<PREFIX>` is `NESTRS` unless the deployment named its own through
//! [`EnvPrefix::VAR`](nest_rs_core::EnvPrefix::VAR); every name in this crate is built
//! from [`var_name`], so the two can never drift.
//!
//! Domain = owning crate's name with the `nest-rs-` prefix stripped. A crate
//! maps **its own** domain; sibling vars may only be borrowed via an
//! **explicit fallback** in a `from_env` (own > borrowed > code default), since
//! the `.env` cascade is merged once before any `from_env` runs.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::EnvPrefix;

use crate::error::ConfigError;
use crate::source::{ConfigSource, EnvSource, MapSource};

/// The fully-qualified name of a namespaced config variable:
/// `<PREFIX>_<DOMAIN>__<KEY>`.
///
/// The primitive [`ConfigService::var_name`] delegates to, exposed for the
/// places that must cite a variable with no reader in hand — a `Validate` impl,
/// a `thiserror` message, a boot check on a pinned struct. Hardcoding the name
/// there would print a variable that does not exist under a custom prefix,
/// which is the one thing an operator reads such a message for.
///
/// ```
/// assert_eq!(nest_rs_config::var_name("database", "URL"), "NESTRS_DATABASE__URL");
/// ```
pub fn var_name(namespace: &str, key: &str) -> String {
    format!(
        "{}_{}__{}",
        EnvPrefix::current(),
        namespace.to_ascii_uppercase(),
        key.to_ascii_uppercase(),
    )
}

/// Which tiers of the environment outrank the value a field falls back to.
///
/// A `Config` is always resolved as *environment over a base*. What the base
/// **is** decides how much of the environment may overrule it, which is the
/// whole of the framework's precedence rule:
/// `real env > pinned in code > .env cascade > in-code defaults`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Precedence {
    /// The base is the config's own defaults, so every tier the source serves
    /// outranks it — the real process env first, then the `.env` cascade.
    #[default]
    OverDefaults,
    /// The base is a value pinned at the call site (`Module::for_root(cfg)`), so
    /// only [`ConfigSource::get_from_deployment`] outranks it. A `.env` file
    /// committed beside the code does not silently undo a deliberate pin; a
    /// deployment variable always does.
    OverPinned,
}

/// Typed reader bound to one namespace; resolves `<PREFIX>_<NAMESPACE>__<KEY>`.
pub struct ConfigService {
    namespace: String,
    source: Arc<dyn ConfigSource>,
    precedence: Precedence,
}

impl ConfigService {
    /// A reader scoped to `namespace`, backed by the process/`.env` environment.
    pub fn for_namespace(namespace: &str) -> Self {
        Self::with_source(namespace, Arc::new(EnvSource))
    }

    /// Build a reader backed by a custom [`ConfigSource`]. The `.env` cascade
    /// is **not** merged — the source is the sole authority for resolution,
    /// and the process env stays untouched (no global side effect from
    /// constructing this reader).
    pub fn with_source(namespace: &str, source: Arc<dyn ConfigSource>) -> Self {
        Self {
            // Stored verbatim: `var_name` uppercases both segments, so casing
            // here would only be a second pass over the same bytes.
            namespace: namespace.to_owned(),
            source,
            precedence: Precedence::OverDefaults,
        }
    }

    /// Narrow this reader to the tiers that outrank a **code-pinned** value.
    /// Called by [`Config::resolve`](crate::Config::resolve) when the call site
    /// passed a config to `Module::for_root`, so a field the deployment sets
    /// still wins while the `.env` cascade defers to the pin.
    pub fn over_pinned(mut self) -> Self {
        self.precedence = Precedence::OverPinned;
        self
    }

    /// Which tiers this reader lets through.
    pub fn precedence(&self) -> Precedence {
        self.precedence
    }

    /// Convenience over [`with_source`](Self::with_source) + [`MapSource`]: a
    /// reader backed by an in-memory map of fully-qualified
    /// `<PREFIX>_<NS>__<KEY>` vars.
    /// Resolves hermetically (no process env, no `.env`), so config tests
    /// and fixtures need no `unsafe { std::env::set_var }`. An empty `vars`
    /// yields all in-code defaults.
    ///
    /// ```
    /// # use nest_rs_config::ConfigService;
    /// let cfg = ConfigService::with_vars("app", [("NESTRS_APP__PORT", "8080")]);
    /// assert_eq!(cfg.get("PORT").as_deref(), Some("8080"));
    /// assert_eq!(cfg.get("MISSING"), None);
    /// ```
    pub fn with_vars<'a>(
        namespace: &str,
        vars: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        Self::with_source(namespace, Arc::new(MapSource::from_iter(vars)))
    }

    /// The full `<PREFIX>_<NAMESPACE>__<KEY>` variable **name** (not its value)
    /// — for error messages and docs that must cite the exact variable.
    pub fn var_name(&self, key: &str) -> String {
        var_name(&self.namespace, key)
    }

    /// The raw string value for `key` in this namespace, or `None` if unset in
    /// every tier this reader's [`Precedence`] lets through.
    pub fn get(&self, key: &str) -> Option<String> {
        let var = self.var_name(key);
        match self.precedence {
            Precedence::OverDefaults => self.source.get(&var),
            Precedence::OverPinned => self.source.get_from_deployment(&var),
        }
    }

    /// `Err` (naming the variable) when set-but-unparseable — boot-fatal, no
    /// silent fallback.
    pub fn parse<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        match self.get(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<T>()
                .map(Some)
                .map_err(|e| ConfigError::parse(self.var_name(key), e.to_string())),
        }
    }

    /// `1`/`true`/`yes`/`on` and their negatives, case-insensitive.
    pub fn flag(&self, key: &str, default: bool) -> Result<bool, ConfigError> {
        match self.get(key) {
            None => Ok(default),
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                other => Err(ConfigError::parse(
                    self.var_name(key),
                    format!("expected a boolean, got `{other}`"),
                )),
            },
        }
    }

    /// Whole seconds, where **`0` means off** — the spelling every long-lived
    /// connection in the framework bounds itself with.
    ///
    /// Unset keeps `base`, `0` is the off/unlimited sentinel, and a
    /// set-but-unparseable value is boot-fatal naming the variable. It lives
    /// here rather than beside any one of them because the sentinel is a
    /// security control — the ceiling on how long a connection replays the
    /// privileges it authenticated with once — and four crates each reading `0`
    /// their own way is four chances for one of them to read it as *zero
    /// seconds* and turn a ceiling into a kill switch.
    pub fn seconds(
        &self,
        key: &str,
        base: Option<Duration>,
    ) -> Result<Option<Duration>, ConfigError> {
        Ok(match self.parse::<u64>(key)? {
            None => base,
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
        })
    }

    /// A whole count, where **`0` means unlimited** — [`seconds`](Self::seconds)'
    /// spelling for a ceiling that bounds a *quantity* rather than a duration.
    ///
    /// Same three cases and the same reason they live here: unset keeps `base`,
    /// `0` is the unlimited sentinel, and set-but-unparseable is boot-fatal
    /// naming the variable. A ceiling on how many units of work one request may
    /// ask for is a security control, and `0` read as *zero allowed* would turn
    /// it into a kill switch — which is exactly the misreading one shared
    /// spelling exists to prevent.
    pub fn count(&self, key: &str, base: Option<usize>) -> Result<Option<usize>, ConfigError> {
        Ok(match self.parse::<usize>(key)? {
            None => base,
            Some(0) => None,
            Some(count) => Some(count),
        })
    }

    /// Comma-separated, trimmed, empties dropped. `default` is the value the
    /// field keeps when the variable is unset — the same shape as
    /// [`flag`](Self::flag), so a `from_env` body passes `base.<field>` and the
    /// overlay reads the same way for every field type.
    pub fn list(&self, key: &str, default: Vec<String>) -> Vec<String> {
        self.get(key)
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or(default)
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    #[test]
    fn var_name_builds_the_namespaced_name() {
        let env = ConfigService::for_namespace("database");
        assert_eq!(env.var_name("URL"), "NESTRS_DATABASE__URL");
        assert_eq!(
            env.var_name("max_connections"),
            "NESTRS_DATABASE__MAX_CONNECTIONS"
        );
    }

    // The readerless primitive must agree with the method, since the two are
    // what an operator compares: an error message built one way and a `.env`
    // line built the other.
    #[test]
    fn the_free_var_name_matches_the_readers() {
        assert_eq!(var_name("database", "url"), "NESTRS_DATABASE__URL");
        assert_eq!(
            var_name("queue", "CONNECT_TIMEOUT_SECS"),
            ConfigService::for_namespace("queue").var_name("connect_timeout_secs"),
        );
    }

    #[test]
    fn parse_reports_the_variable_on_failure() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__MAX", "not-a-number");
            let env = ConfigService::for_namespace("testdb");
            let err = env.parse::<u32>("MAX").expect_err("non-numeric must fail");
            assert!(
                matches!(err, ConfigError::Parse { ref var, .. } if var == "NESTRS_TESTDB__MAX")
            );
            Ok(())
        });
    }

    #[test]
    fn parse_is_none_when_unset() {
        figment::Jail::expect_with(|_| {
            let env = ConfigService::for_namespace("testdb");
            assert!(
                env.parse::<u32>("UNSET_KEY")
                    .expect("unset is Ok(None)")
                    .is_none()
            );
            Ok(())
        });
    }

    #[test]
    fn flag_reads_common_spellings() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTF__ON", "yes");
            jail.set_env("NESTRS_TESTF__OFF", "false");
            let env = ConfigService::for_namespace("testf");
            assert!(env.flag("ON", false).unwrap());
            assert!(!env.flag("OFF", true).unwrap());
            assert!(env.flag("MISSING", true).unwrap());
            Ok(())
        });
    }

    #[test]
    fn list_splits_on_commas() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTL__SCOPES", "read:user, write , ,admin");
            let env = ConfigService::for_namespace("testl");
            assert_eq!(
                env.list("SCOPES", Vec::new()),
                vec!["read:user", "write", "admin"],
            );
            Ok(())
        });
    }

    #[test]
    fn list_keeps_the_default_when_unset() {
        let env = ConfigService::with_vars("testl", []);
        assert_eq!(
            env.list("SCOPES", vec!["pinned".to_owned()]),
            vec!["pinned".to_owned()],
            "an unset list keeps the base, the same way `flag` keeps its default",
        );
    }

    // The precedence split D-2 rests on: a pinned value loses to a deployment
    // variable and wins over the `.env` cascade. `MapSource` stands in for a
    // custom source, whose default is "deployment-supplied" — the fail-safe
    // direction, so a Vault value is never shadowed by a pinned struct.
    #[test]
    fn over_pinned_narrows_to_the_deployment_tier() {
        let env = ConfigService::with_vars("prec", [("NESTRS_PREC__PORT", "9000")]);
        assert_eq!(env.get("PORT").as_deref(), Some("9000"));
        assert_eq!(
            env.over_pinned().get("PORT").as_deref(),
            Some("9000"),
            "a custom source is deployment-supplied unless it says otherwise",
        );
    }

    #[test]
    fn env_source_over_pinned_ignores_the_dotenv_cascade_but_not_the_real_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "NESTRS_PRECPIN__FROM_FILE=from_dotenv")?;
            jail.set_env("NESTRS_PRECPIN__FROM_REAL", "from_real");
            let pinned = ConfigService::for_namespace("precpin").over_pinned();
            assert_eq!(
                pinned.get("FROM_REAL").as_deref(),
                Some("from_real"),
                "a deployment variable outranks a value pinned in code",
            );
            assert_eq!(
                pinned.get("FROM_FILE"),
                None,
                "a committed .env file does not silently undo a deliberate pin",
            );
            // Unpinned, the cascade is back in play.
            assert_eq!(
                ConfigService::for_namespace("precpin")
                    .get("FROM_FILE")
                    .as_deref(),
                Some("from_dotenv"),
            );
            Ok(())
        });
    }

    // A `with_source` reader bypasses the env entirely — pin that the source
    // is the sole authority so a third-party Vault/ConfigMap impl is not
    // shadowed by stale process env.
    #[test]
    fn with_source_reads_from_the_custom_source_only() {
        use std::collections::HashMap;
        struct Map(HashMap<&'static str, &'static str>);
        impl ConfigSource for Map {
            fn get(&self, var: &str) -> Option<String> {
                self.0.get(var).map(|s| (*s).to_owned())
            }
        }
        let source = Arc::new(Map(HashMap::from([(
            "NESTRS_CUSTOM__URL",
            "value-from-map",
        )])));
        let env = ConfigService::with_source("custom", source);
        assert_eq!(env.get("URL").as_deref(), Some("value-from-map"));
        assert!(env.get("MISSING").is_none());
    }

    // The dotenv cascade used to fire from `for_namespace`, which meant any
    // `ConfigService` — including one built on a custom source — would
    // permanently merge `.env` into the process env. Pin that a non-env
    // source never triggers the merge: `.env` exists in the jail with a
    // marker, and after a `with_source` read, that marker must still be
    // unset in `std::env`.
    #[test]
    fn with_source_does_not_load_dotenv_into_process_env() {
        struct Empty;
        impl ConfigSource for Empty {
            fn get(&self, _var: &str) -> Option<String> {
                None
            }
        }
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "NESTRS_LEAK_GUARD__SHOULD_STAY_UNSET=loaded-from-dotenv",
            )?;
            // Build + use the custom-source reader. If dotenv leaked here it
            // would set the marker in the jailed process env.
            let env = ConfigService::with_source("leakguard", Arc::new(Empty));
            assert!(env.get("ANYTHING").is_none());
            assert!(
                std::env::var("NESTRS_LEAK_GUARD__SHOULD_STAY_UNSET").is_err(),
                "custom-source path must not merge .env into the process env",
            );
            Ok(())
        });
    }

    /// `count` and `seconds` are one spelling of one sentinel; the tests are
    /// paired for the same reason the doc comments are — a divergence would mean
    /// `0` reading as *off* on one and *zero allowed* on the other, which is the
    /// misreading the shared helper exists to prevent.
    #[test]
    fn count_and_seconds_read_zero_as_the_same_sentinel() {
        let base_count = Some(5usize);
        let base_secs = Some(Duration::from_secs(5));

        let unset = ConfigService::with_vars("probe", []);
        assert_eq!(unset.count("N", base_count).expect("unset"), base_count);
        assert_eq!(unset.seconds("N", base_secs).expect("unset"), base_secs);

        let zero = ConfigService::with_vars("probe", [("NESTRS_PROBE__N", "0")]);
        assert_eq!(
            zero.count("N", base_count).expect("zero"),
            None,
            "`0` is the unlimited sentinel, never a ceiling of zero",
        );
        assert_eq!(zero.seconds("N", base_secs).expect("zero"), None);

        let set = ConfigService::with_vars("probe", [("NESTRS_PROBE__N", "7")]);
        assert_eq!(set.count("N", base_count).expect("set"), Some(7));

        for bad in ["-1", " 7 ", "abc", "3.0"] {
            let service = ConfigService::with_vars("probe", [("NESTRS_PROBE__N", bad)]);
            let err = service
                .count("N", base_count)
                .expect_err("a set-but-unparseable ceiling is boot-fatal");
            assert!(
                err.to_string().contains("NESTRS_PROBE__N"),
                "and it names the variable: {err}",
            );
        }
    }
}
