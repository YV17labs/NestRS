//! [`StaticFilesConfig`] — where the files answer, which directory is read,
//! the single-page-app fallback and the cache lifetime, from
//! `<PREFIX>_STATIC_FILES__*` or pinned through
//! [`StaticFilesModule::for_root`](crate::StaticFilesModule::for_root).

use std::path::PathBuf;
use std::time::Duration;

use nest_rs_config::{
    Bound, BoundedDuration, Config, ConfigError, ConfigService, DurationBounds, Floor, Result,
    config,
};

/// How long a client may reuse a file before it asks again, the variable that
/// sets it, and why its range ends where it does.
pub(crate) const MAX_AGE: DurationBounds = DurationBounds::secs(
    "MAX_AGE_SECS",
    "StaticFilesConfig::max_age",
    Floor::UnitsOrOff(Bound {
        count: 1,
        why: "a lifetime under a second is a revalidation on every request, which is what \
              leaving it off already says",
    }),
    Bound {
        count: 365 * 24 * 60 * 60,
        why: "a year is the longest freshness HTTP has ever asked a cache to honour (RFC 2616 \
              §14.21), and caches treat it as forever, so a longer one is a unit slip",
    },
);

/// Where the files answer and how they are cached, settable through
/// `<PREFIX>_STATIC_FILES__*` or pinned in code, field by field.
#[config(namespace = "static_files")]
#[derive(Clone, Debug)]
pub struct StaticFilesConfig {
    /// The directory served (key `ROOT`), relative to the working directory.
    /// Resolved at boot, and refused there when unset, missing or not a
    /// directory — and when set beside an [`Embedded`](crate::Embedded) source,
    /// which has no directory to read.
    pub root: Option<PathBuf>,
    /// The URL path the files answer under (key `PATH`, default `/`). Literal
    /// and outside the global prefix; every route and self-mount answers first.
    pub path: String,
    /// Answer a browser's navigation to a path no file matches with the root's
    /// `index.html` (key `SPA_FALLBACK`, default `false`), so a single-page
    /// app's client-side routes load. Never for a request that did not ask for
    /// a page, nor for a path whose last segment names a file.
    pub spa_fallback: bool,
    /// How long a client may reuse a file without asking again (key
    /// `MAX_AGE_SECS`, `0` for unset): `public, max-age` on every file but an
    /// HTML document. Unset, the default, every file is revalidated
    /// (`no-cache`), which suits a build whose file names do not change.
    pub max_age: Option<Duration>,
}

impl Default for StaticFilesConfig {
    fn default() -> Self {
        Self {
            root: None,
            path: "/".to_owned(),
            spa_fallback: false,
            max_age: None,
        }
    }
}

impl Config for StaticFilesConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let path = env.get("PATH")?.unwrap_or(base.path);
        let path = nest_rs_http::literal_mount_path(&path).ok_or_else(|| {
            ConfigError::parse(
                env.var_name("PATH"),
                "must be a literal URL path such as `/` or `/assets`, which the router matches as \
                 written: no empty, `.` or `..` segment, no pattern syntax (`:` `*` `<`), and none \
                 of `%` `?` `#` `\\` — set here, or as `StaticFilesConfig::path` in code",
            )
        })?;
        Ok(Self {
            root: env.get("ROOT")?.map(PathBuf::from).or(base.root),
            path,
            spa_fallback: env.flag("SPA_FALLBACK", base.spa_fallback)?,
            max_age: MAX_AGE
                .read_optional(env, base.max_age)?
                .map(|read: BoundedDuration| read.value),
        })
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::{Namespaced, var_name};

    use super::*;

    fn read(vars: &[(&str, &str)], base: StaticFilesConfig) -> Result<StaticFilesConfig> {
        StaticFilesConfig::from_env(
            &ConfigService::with_vars(
                StaticFilesConfig::NAMESPACE,
                vars.iter().map(|(k, v)| (*k, *v)),
            ),
            base,
        )
    }

    fn refused_under(err: &ConfigError, key: &str) -> bool {
        matches!(err, ConfigError::Parse { var, .. } if *var == var_name(StaticFilesConfig::NAMESPACE, key))
    }

    #[test]
    fn the_namespace_is_the_crate_s_stem() {
        assert_eq!(StaticFilesConfig::NAMESPACE, "static_files");
    }

    #[test]
    fn unset_keeps_the_defaults() {
        let config = read(&[], StaticFilesConfig::default()).unwrap();
        assert_eq!(config.root, None);
        assert_eq!(config.path, "/");
        assert!(!config.spa_fallback);
        assert_eq!(config.max_age, None);
    }

    #[test]
    fn each_field_is_set_from_its_variable() {
        let config = read(
            &[
                ("ROOT", "web/dist"),
                ("PATH", "/app/"),
                ("SPA_FALLBACK", "true"),
                ("MAX_AGE_SECS", "600"),
            ],
            StaticFilesConfig::default(),
        )
        .unwrap();
        assert_eq!(config.root, Some(PathBuf::from("web/dist")));
        assert_eq!(config.path, "/app", "the path is canonical");
        assert!(config.spa_fallback);
        assert_eq!(config.max_age, Some(Duration::from_secs(600)));
    }

    #[test]
    fn each_field_is_set_in_code_and_the_environment_overlays_it() {
        let pinned = StaticFilesConfig {
            root: Some("public".into()),
            path: "/assets".into(),
            spa_fallback: true,
            max_age: Some(Duration::from_secs(60)),
        };
        let config = read(&[], pinned.clone()).unwrap();
        assert_eq!(config.root, pinned.root);
        assert_eq!(config.path, "/assets");
        assert!(config.spa_fallback);
        assert_eq!(config.max_age, pinned.max_age);

        let config = read(&[("MAX_AGE_SECS", "0")], pinned).unwrap();
        assert_eq!(config.max_age, None, "`0` turns the lifetime off");
    }

    #[test]
    fn a_max_age_outside_its_range_is_refused_from_either_side() {
        let past = (MAX_AGE.most().count + 1).to_string();
        let err = read(&[("MAX_AGE_SECS", &past)], StaticFilesConfig::default()).unwrap_err();
        assert!(refused_under(&err, "MAX_AGE_SECS"), "{err}");

        let pinned = StaticFilesConfig {
            max_age: Some(Duration::from_secs(MAX_AGE.most().count + 1)),
            ..StaticFilesConfig::default()
        };
        let err = read(&[], pinned).unwrap_err();
        assert!(refused_under(&err, "MAX_AGE_SECS"), "{err}");
        assert!(
            err.to_string().contains("StaticFilesConfig::max_age"),
            "{err}"
        );

        let zero = StaticFilesConfig {
            max_age: Some(Duration::ZERO),
            ..StaticFilesConfig::default()
        };
        let err = read(&[], zero).unwrap_err();
        assert!(
            refused_under(&err, "MAX_AGE_SECS"),
            "off in code is `None`: {err}"
        );
    }

    #[test]
    fn a_pattern_in_the_path_is_refused_from_either_side() {
        for raw in [
            "/assets/*rest",
            "/:id",
            "/a//b",
            "/a/../b",
            "/a%2fb",
            "/a b",
        ] {
            let err = read(&[("PATH", raw)], StaticFilesConfig::default()).unwrap_err();
            assert!(refused_under(&err, "PATH"), "{raw}: {err}");
            assert!(
                !err.to_string().contains(raw),
                "never quotes the value: {err}"
            );

            let pinned = StaticFilesConfig {
                path: raw.to_owned(),
                ..StaticFilesConfig::default()
            };
            let err = read(&[], pinned).unwrap_err();
            assert!(err.to_string().contains("StaticFilesConfig::path"), "{err}");
        }
    }
}
