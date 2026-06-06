//! [`OpenApiConfig`] — the OpenAPI document `info` block, populated from
//! `NESTRS_OPENAPI__*` in the `.env` cascade.

use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

#[config(namespace = "openapi")]
#[derive(Clone, Debug, Validate)]
pub struct OpenApiConfig {
    pub title: String,
    pub version: String,
    pub description: Option<String>,
}

impl Default for OpenApiConfig {
    fn default() -> Self {
        Self {
            title: "nestrs API".into(),
            version: "0.1.0".into(),
            description: None,
        }
    }
}

impl Config for OpenApiConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        let d = Self::default();
        Ok(Self {
            title: env.get("TITLE").unwrap_or(d.title),
            version: env.get("VERSION").unwrap_or(d.version),
            description: env.get("DESCRIPTION"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_non_empty_strings() {
        let d = OpenApiConfig::default();
        assert!(!d.title.is_empty());
        assert!(!d.version.is_empty());
        assert!(d.description.is_none());
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
                ("NESTRS_OPENAPI__TITLE", None),
                ("NESTRS_OPENAPI__VERSION", None),
                ("NESTRS_OPENAPI__DESCRIPTION", None),
            ],
            || {
                let cfg = OpenApiConfig::from_env(&ConfigService::for_namespace("openapi"))
                    .expect("ok");
                let d = OpenApiConfig::default();
                assert_eq!(cfg.title, d.title);
                assert_eq!(cfg.version, d.version);
                assert!(cfg.description.is_none());
            },
        );
    }

    #[test]
    fn from_env_overrides_each_field_independently() {
        with_env(
            &[
                ("NESTRS_OPENAPI__TITLE", Some("Custom API")),
                ("NESTRS_OPENAPI__VERSION", Some("9.9.9")),
                ("NESTRS_OPENAPI__DESCRIPTION", Some("Generated docs")),
            ],
            || {
                let cfg = OpenApiConfig::from_env(&ConfigService::for_namespace("openapi"))
                    .expect("ok");
                assert_eq!(cfg.title, "Custom API");
                assert_eq!(cfg.version, "9.9.9");
                assert_eq!(cfg.description.as_deref(), Some("Generated docs"));
            },
        );
    }
}
