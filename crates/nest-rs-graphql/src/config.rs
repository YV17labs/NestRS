//! [`GraphqlConfig`] — loaded from `NESTRS_GRAPHQL__*`. Every field defaults
//! production-safe (playground off, SDL emit off, no anti-DoS limits); an
//! `.env.development` opts the tooling in and an app's `module.rs` pins the
//! production limits so `app.rs` carries no config literal.

use std::path::PathBuf;

use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

pub(crate) const DEFAULT_PATH: &str = "/graphql";

#[config(namespace = "graphql")]
#[derive(Clone, Debug, Validate)]
pub struct GraphqlConfig {
    /// Default `/graphql`.
    pub path: String,
    /// Default `false` (production-safe).
    pub playground: bool,
    /// Where the committed SDL lives. Default `schema.graphql`.
    pub schema_path: PathBuf,
    /// (Re)write `schema_path` from the live schema once at boot. Default
    /// `false`. A write failure is logged, never fatal.
    pub emit_sdl: bool,
    /// Maximum nesting depth of an incoming query AST. `None` (the default)
    /// disables the check — opt in by setting `NESTRS_GRAPHQL__MAX_DEPTH` or
    /// pinning the field. A sensible production value is in the 10-20 range:
    /// caps recursive bombs (`{ a { a { a { … } } } }`) without rejecting
    /// legitimate nested queries. Cheap to enforce (one AST walk).
    ///
    /// `Some(0)` is rejected at boot: async-graphql checks `depth > limit`
    /// strictly and every field has depth ≥ 1, so `0` would brick every
    /// query. Use `None` to disable.
    #[validate(range(min = 1))]
    pub max_depth: Option<usize>,
    /// Maximum complexity score of an incoming query AST. `None` (the default)
    /// disables the check — opt in by setting `NESTRS_GRAPHQL__MAX_COMPLEXITY`
    /// or pinning the field. Score = 1 per field + per-field overrides emitted
    /// by `#[expose]` on list relations (multiplier on the unbounded fanout).
    /// A sensible production value sits in the 1000-5000 range and should be
    /// tuned from observed legitimate queries.
    ///
    /// `Some(0)` is rejected at boot for the same reason as `max_depth`.
    #[validate(range(min = 1))]
    pub max_complexity: Option<usize>,
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.into(),
            playground: false,
            schema_path: "schema.graphql".into(),
            emit_sdl: false,
            max_depth: None,
            max_complexity: None,
        }
    }
}

impl Config for GraphqlConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        let d = Self::default();
        Ok(Self {
            path: env.get("PATH").unwrap_or(d.path),
            playground: env.flag("PLAYGROUND", d.playground)?,
            schema_path: env
                .get("SCHEMA_PATH")
                .map(PathBuf::from)
                .unwrap_or(d.schema_path),
            emit_sdl: env.flag("EMIT_SDL", d.emit_sdl)?,
            max_depth: env.parse("MAX_DEPTH")?,
            max_complexity: env.parse("MAX_COMPLEXITY")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_production_safe() {
        let d = GraphqlConfig::default();
        assert_eq!(d.path, "/graphql");
        assert!(!d.playground, "playground exposed in prod is a CVE");
        assert!(!d.emit_sdl, "writing SDL from prod is unwanted side effect");
        assert_eq!(d.schema_path, PathBuf::from("schema.graphql"));
        assert!(
            d.max_depth.is_none(),
            "max_depth defaults None — opt-in keeps the change backward-compatible",
        );
        assert!(d.max_complexity.is_none());
    }

    #[test]
    fn default_path_constant_pins_the_mount_point() {
        // App code reads this path string indirectly through the module — a
        // rename here breaks every reverse proxy.
        assert_eq!(DEFAULT_PATH, "/graphql");
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            match v {
                Some(value) => unsafe { std::env::set_var(k, value) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        for (k, _) in vars {
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    #[test]
    fn from_env_falls_back_to_defaults_when_unset() {
        with_env(
            &[
                ("NESTRS_GRAPHQL__PATH", None),
                ("NESTRS_GRAPHQL__PLAYGROUND", None),
                ("NESTRS_GRAPHQL__SCHEMA_PATH", None),
                ("NESTRS_GRAPHQL__EMIT_SDL", None),
                ("NESTRS_GRAPHQL__MAX_DEPTH", None),
                ("NESTRS_GRAPHQL__MAX_COMPLEXITY", None),
            ],
            || {
                let cfg =
                    GraphqlConfig::from_env(&ConfigService::for_namespace("graphql")).expect("ok");
                let d = GraphqlConfig::default();
                assert_eq!(cfg.path, d.path);
                assert_eq!(cfg.playground, d.playground);
                assert_eq!(cfg.schema_path, d.schema_path);
                assert_eq!(cfg.emit_sdl, d.emit_sdl);
                assert_eq!(cfg.max_depth, d.max_depth);
                assert_eq!(cfg.max_complexity, d.max_complexity);
            },
        );
    }

    #[test]
    fn validate_rejects_zero_limits_so_some_zero_does_not_brick_the_endpoint() {
        // async-graphql's depth/complexity check is strict `>`, and every
        // non-empty selection has depth ≥ 1, so `Some(0)` would reject every
        // query at boot — a footgun the validator must catch.
        let zero_depth = GraphqlConfig {
            max_depth: Some(0),
            ..GraphqlConfig::default()
        };
        assert!(
            zero_depth.validate().is_err(),
            "Some(0) must fail validation — none of the documented `disable` opts is `0`"
        );
        let zero_complexity = GraphqlConfig {
            max_complexity: Some(0),
            ..GraphqlConfig::default()
        };
        assert!(zero_complexity.validate().is_err());
        // Sanity: Some(1) is meaningfully tight but legal; defaults are fine.
        let tight = GraphqlConfig {
            max_depth: Some(1),
            max_complexity: Some(1),
            ..GraphqlConfig::default()
        };
        assert!(tight.validate().is_ok());
        assert!(GraphqlConfig::default().validate().is_ok());
    }

    #[test]
    fn from_env_reads_each_field_when_set() {
        with_env(
            &[
                ("NESTRS_GRAPHQL__PATH", Some("/api/graphql")),
                ("NESTRS_GRAPHQL__PLAYGROUND", Some("true")),
                ("NESTRS_GRAPHQL__SCHEMA_PATH", Some("./schema-out.graphql")),
                ("NESTRS_GRAPHQL__EMIT_SDL", Some("true")),
                ("NESTRS_GRAPHQL__MAX_DEPTH", Some("15")),
                ("NESTRS_GRAPHQL__MAX_COMPLEXITY", Some("2000")),
            ],
            || {
                let cfg =
                    GraphqlConfig::from_env(&ConfigService::for_namespace("graphql")).expect("ok");
                assert_eq!(cfg.path, "/api/graphql");
                assert!(cfg.playground);
                assert_eq!(cfg.schema_path, PathBuf::from("./schema-out.graphql"));
                assert!(cfg.emit_sdl);
                assert_eq!(cfg.max_depth, Some(15));
                assert_eq!(cfg.max_complexity, Some(2000));
            },
        );
    }
}
