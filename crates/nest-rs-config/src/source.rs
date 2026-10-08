//! [`ConfigSource`] — pluggable backing store for [`ConfigService`].
//!
//! [`EnvSource`] (default) resolves each variable from the real process
//! environment, falling back to the parsed `.env` cascade. A third-party crate
//! ships an alternative by implementing [`ConfigSource`] and constructing
//! [`ConfigService::with_source`].
//!
//! `Config::from_env` runs sync at boot, so a remote source pre-fetches into an
//! in-memory map and serves `get` from that map.
//!
//! [`ConfigService`]: crate::ConfigService
//! [`ConfigService::with_source`]: crate::ConfigService::with_source

use std::collections::HashMap;
use std::env;

use crate::dotenv::dotenv_values;

/// Resolve `name` from the real process environment, falling back to the parsed
/// `.env` cascade. The real env always wins; a value **present but empty** in
/// the real env counts as unset **and** suppresses the dotenv fallback. Never
/// mutates the process environment.
pub fn env_var(name: &str) -> Option<String> {
    crate::unclaimed::witness(name);
    env_var_from(name, dotenv_values())
}

/// Core of [`env_var`], with the dotenv map supplied.
#[expect(
    clippy::disallowed_methods,
    reason = "the config loader is the one reader of the process environment"
)]
fn env_var_from(name: &str, dotenv: &HashMap<String, String>) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        Ok(_) => None,
        Err(env::VarError::NotUnicode(_)) => {
            tracing::warn!(
                target: crate::TARGET,
                name,
                "environment variable is not valid UTF-8 — treated as unset, cascade suppressed",
            );
            None
        }
        Err(env::VarError::NotPresent) => dotenv.get(name).filter(|v| !v.is_empty()).cloned(),
    }
}

/// Read `name` from the **real** process environment only — no `.env` fallback.
/// Empty counts as unset. `<PREFIX>_ENV` is read here: through [`env_var`] it
/// would recurse into `dotenv_values`, which reads it.
#[expect(
    clippy::disallowed_methods,
    reason = "the config loader is the one reader of the process environment"
)]
pub(crate) fn real_env_var(name: &str) -> Option<String> {
    real_env_var_from(name, env::var(name))
}

/// Core of [`real_env_var`], with the read supplied so the non-UTF-8 branch is
/// testable without `unsafe`.
fn real_env_var_from(name: &str, read: Result<String, env::VarError>) -> Option<String> {
    match read {
        Ok(v) if !v.is_empty() => Some(v),
        Ok(_) | Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => {
            tracing::warn!(
                target: crate::TARGET,
                name,
                "environment variable is not valid UTF-8 — treated as unset",
            );
            None
        }
    }
}

/// Read `name` from the **deployment** — the real process environment minus
/// anything `Environment::init` merged in from a cascade file, which would
/// otherwise outrank a `for_root` pin.
pub(crate) fn deployment_env_var(name: &str) -> Option<String> {
    if crate::dotenv::published_from_cascade(name) {
        return None;
    }
    real_env_var(name)
}

/// Where a [`ConfigService`](crate::ConfigService) reads raw values from. The
/// default is [`EnvSource`] (process env + `.env` cascade); an alternative is
/// passed to [`ConfigService::with_source`](crate::ConfigService::with_source).
pub trait ConfigSource: Send + Sync + 'static {
    /// Return the raw value for the fully-qualified variable name (e.g.
    /// `"<PREFIX>_SEAORM__URL"`). Empty strings should be treated as unset.
    fn get(&self, var: &str) -> Option<String>;

    /// The subset of [`get`](Self::get) that comes from the **deployment** —
    /// the tier that outranks a value pinned in code by `Module::for_root(cfg)`.
    ///
    /// Defaults to [`get`](Self::get): a custom source is deployment-supplied
    /// unless it says otherwise, so a pinned struct never shadows it.
    fn get_from_deployment(&self, var: &str) -> Option<String> {
        self.get(var)
    }

    /// Whether the **deployment** names `var` at all — an empty value included,
    /// which is how a deployment unsets a value a lower tier carries.
    ///
    /// When either spelling of a key answers `true`, neither is read from a lower
    /// tier. Defaults to whether
    /// [`get_from_deployment`](Self::get_from_deployment) holds a value.
    fn in_deployment(&self, var: &str) -> bool {
        self.get_from_deployment(var).is_some()
    }
}

/// Default [`ConfigSource`] — resolves from the real process environment with a
/// parsed `.env` cascade fallback (real env wins).
#[derive(Default)]
pub struct EnvSource;

impl ConfigSource for EnvSource {
    fn get(&self, var: &str) -> Option<String> {
        env_var(var)
    }

    /// The real process environment only, minus what `Environment::init` merged
    /// in from the cascade.
    fn get_from_deployment(&self, var: &str) -> Option<String> {
        deployment_env_var(var)
    }

    /// Present in the real process environment, empty included, and not merged
    /// there from the cascade by `Environment::init`.
    #[expect(
        clippy::disallowed_methods,
        reason = "the config loader is the one reader of the process environment"
    )]
    fn in_deployment(&self, var: &str) -> bool {
        std::env::var_os(var).is_some() && !crate::dotenv::published_from_cascade(var)
    }
}

/// A [`ConfigSource`] backed by an in-memory map, touching neither the process
/// environment nor the `.env` cascade. Keys are the fully-qualified
/// `<PREFIX>_<DOMAIN>__<KEY>` names.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_config::{ConfigService, MapSource, var_name};
///
/// // Built, never spelled: a fixture keyed on a literal reads nothing under a
/// // deployment that renamed the prefix, and reads it as "unset".
/// let port = var_name("app", "PORT");
/// let source = MapSource::from_iter([(port.as_str(), "8080")]);
/// let cfg = ConfigService::with_source("app", Arc::new(source));
/// assert_eq!(cfg.get("PORT")?.as_deref(), Some("8080"));
/// assert_eq!(cfg.get("MISSING")?, None); // absent ⇒ falls back to in-code defaults
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct MapSource(HashMap<String, String>);

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for MapSource {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl ConfigSource for MapSource {
    fn get(&self, var: &str) -> Option<String> {
        self.0.get(var).filter(|v| !v.is_empty()).cloned()
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use super::*;

    /// The deployment tier — what a config pinned in code reads — reports a
    /// value it cannot decode instead of dropping it in silence.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_deployment_variable_is_reported_not_silently_unset() {
        use std::os::unix::ffi::OsStringExt;

        let logs = nest_rs_testing::LogCapture::install();
        let read = Err(env::VarError::NotUnicode(std::ffi::OsString::from_vec(
            vec![0x66, 0xff],
        )));
        assert_eq!(real_env_var_from("FIXTURE_DEPLOY__BYTES", read), None);
        let event = logs
            .find(
                crate::TARGET,
                "environment variable is not valid UTF-8 — treated as unset",
            )
            .into_iter()
            .next()
            .expect("an undecodable deployment value is reported");
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("name").as_deref(),
            Some("FIXTURE_DEPLOY__BYTES"),
            "{event:?}"
        );
    }

    #[test]
    fn env_var_prefers_real_env_over_dotenv() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("FIXTURE_PREC__X", "from_real");
            let map = HashMap::from([("FIXTURE_PREC__X".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(
                env_var_from("FIXTURE_PREC__X", &map).as_deref(),
                Some("from_real"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_falls_back_to_dotenv_when_real_env_absent() {
        figment::Jail::expect_with(|_| {
            let map = HashMap::from([("FIXTURE_PREC__Y".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(
                env_var_from("FIXTURE_PREC__Y", &map).as_deref(),
                Some("from_dotenv"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_present_but_empty_real_env_suppresses_dotenv_fallback() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("FIXTURE_PREC__Z", "");
            let map = HashMap::from([("FIXTURE_PREC__Z".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(env_var_from("FIXTURE_PREC__Z", &map), None);
            Ok(())
        });
    }

    #[test]
    fn deployment_env_var_ignores_values_published_from_the_cascade() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "FIXTURE_DEPLOY__FROM_FILE=file\nFIXTURE_DEPLOY__FROM_REAL=file",
            )?;
            jail.set_env("FIXTURE_DEPLOY__FROM_REAL", "real");
            crate::dotenv::load_cascade(std::path::Path::new("."), crate::Environment::Development);

            assert_eq!(
                deployment_env_var("FIXTURE_DEPLOY__FROM_FILE"),
                None,
                "a committed `.env` must lose to a value pinned in `for_root`",
            );
            assert_eq!(
                deployment_env_var("FIXTURE_DEPLOY__FROM_REAL").as_deref(),
                Some("real"),
                "a real deployment variable still outranks the pin",
            );
            assert_eq!(
                EnvSource.get("FIXTURE_DEPLOY__FROM_FILE").as_deref(),
                Some("file"),
            );
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts the process environment was left alone"
    )]
    fn env_var_read_never_writes_the_dotenv_value_into_the_process_env() {
        figment::Jail::expect_with(|_| {
            let map = HashMap::from([("FIXTURE_PREC__ONLY_IN_MAP".to_owned(), "v".to_owned())]);
            assert_eq!(
                env_var_from("FIXTURE_PREC__ONLY_IN_MAP", &map).as_deref(),
                Some("v"),
            );
            assert!(std::env::var("FIXTURE_PREC__ONLY_IN_MAP").is_err());
            Ok(())
        });
    }
}
