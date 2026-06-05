//! [`GraphqlConfig`] — loaded from `NESTRS_GRAPHQL__*`. Every field defaults
//! production-safe (playground off, SDL emit off); `.env.development` opts the
//! tooling in so `app.rs` carries no config literal.

use std::path::PathBuf;

use nestrs_config::{Config, ConfigService, Result, config};
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
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.into(),
            playground: false,
            schema_path: "schema.graphql".into(),
            emit_sdl: false,
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
            ],
            || {
                let cfg = GraphqlConfig::from_env(&ConfigService::for_namespace("graphql"))
                    .expect("ok");
                let d = GraphqlConfig::default();
                assert_eq!(cfg.path, d.path);
                assert_eq!(cfg.playground, d.playground);
                assert_eq!(cfg.schema_path, d.schema_path);
                assert_eq!(cfg.emit_sdl, d.emit_sdl);
            },
        );
    }

    #[test]
    fn from_env_reads_each_field_when_set() {
        with_env(
            &[
                ("NESTRS_GRAPHQL__PATH", Some("/api/graphql")),
                ("NESTRS_GRAPHQL__PLAYGROUND", Some("true")),
                ("NESTRS_GRAPHQL__SCHEMA_PATH", Some("./schema-out.graphql")),
                ("NESTRS_GRAPHQL__EMIT_SDL", Some("true")),
            ],
            || {
                let cfg = GraphqlConfig::from_env(&ConfigService::for_namespace("graphql"))
                    .expect("ok");
                assert_eq!(cfg.path, "/api/graphql");
                assert!(cfg.playground);
                assert_eq!(cfg.schema_path, PathBuf::from("./schema-out.graphql"));
                assert!(cfg.emit_sdl);
            },
        );
    }
}
