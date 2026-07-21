//! [`OpenApiConfig`] — the OpenAPI document `info` block, populated from
//! `NESTRS_OPENAPI__*` in the `.env` cascade.

use std::path::PathBuf;

use nest_rs_config::{Config, ConfigService, Environment, Result, config};
use validator::Validate;

/// The OpenAPI document's `info` block plus the master enable switch, settable
/// via `NESTRS_OPENAPI__*` or pinned through
/// [`OpenApiModule::for_root`](crate::OpenApiModule::for_root).
#[config(namespace = "openapi")]
#[derive(Clone, Debug, Validate)]
pub struct OpenApiConfig {
    /// Master switch for the documentation endpoints.
    ///
    /// Both `/api-json` (the document) and `/api` (Swagger UI) self-mount
    /// `EdgePosture::Exempt` — deliberately **public**, no auth — so while
    /// enabled the full document (every path, parameter, and schema linked into
    /// the binary) is served to any anonymous caller. Because that surface is
    /// public and unauthenticated, [`from_env`](Config::from_env) defaults it
    /// **OFF outside a dev/test profile** (HTTP-S5): a dev run keeps the docs on
    /// for ergonomics; staging/production must opt in with
    /// `NESTRS_OPENAPI__ENABLED=true`, which is honored but logged loudly at
    /// boot. When `false`, [`OpenApiModule`](crate::OpenApiModule) mounts neither
    /// endpoint. A set-but-unparseable `NESTRS_OPENAPI__ENABLED` fails boot
    /// naming the variable — it never silently falls back to on. (The struct
    /// `Default` stays `true` for the pinned-config / dev path.)
    pub enabled: bool,
    /// The API title shown in the document `info` block and Swagger UI.
    pub title: String,
    /// The API version string in the `info` block (the app's version, not nestrs').
    pub version: String,
    /// Optional long-form API description for the `info` block.
    pub description: Option<String>,
    /// (Re)write [`document_path`](Self::document_path) with the built document
    /// once at boot — the OpenAPI analogue of the GraphQL SDL emit, so the
    /// committed `openapi.json` stays fresh as a side effect of a dev run.
    /// Default `false`; the demo turns it on with `NESTRS_OPENAPI__EMIT_DOCUMENT=true`.
    pub emit_document: bool,
    /// Where [`emit_document`](Self::emit_document) writes the JSON document,
    /// relative to the process working directory. Default `openapi.json`.
    pub document_path: PathBuf,
}

impl Default for OpenApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            title: "nestrs API".into(),
            version: "0.1.0".into(),
            description: None,
            emit_document: false,
            document_path: "openapi.json".into(),
        }
    }
}

impl Config for OpenApiConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        let d = Self::default();
        let environment = Environment::from_env();
        // Secure-by-default (HTTP-S5): the docs endpoints are public and
        // unauthenticated, so outside a dev/test profile they default OFF. `flag`
        // still returns `Err` (naming the var) on a set-but-unparseable value, so
        // a typo'd `NESTRS_OPENAPI__ENABLED` stays boot-fatal — it never silently
        // falls back to on.
        let enabled = env.flag("ENABLED", docs_default_enabled(environment))?;
        if enabled && !docs_default_enabled(environment) {
            tracing::warn!(
                target: "nest_rs::openapi",
                environment = environment.as_str(),
                "OpenAPI documentation endpoints are enabled and public outside a dev profile",
            );
        }
        Ok(Self {
            enabled,
            title: env.get("TITLE").unwrap_or(d.title),
            version: env.get("VERSION").unwrap_or(d.version),
            description: env.get("DESCRIPTION"),
            emit_document: env.flag("EMIT_DOCUMENT", d.emit_document)?,
            document_path: env
                .get("DOCUMENT_PATH")
                .map(PathBuf::from)
                .unwrap_or(d.document_path),
        })
    }
}

/// HTTP-S5: the public, unauthenticated docs endpoints default ON only in a
/// dev/test profile — OFF in staging/production unless explicitly enabled.
fn docs_default_enabled(environment: Environment) -> bool {
    !matches!(environment, Environment::Production | Environment::Staging)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_non_empty_strings() {
        let d = OpenApiConfig::default();
        assert!(d.enabled, "docs are on by default for dev ergonomics");
        assert!(!d.title.is_empty());
        assert!(!d.version.is_empty());
        assert!(d.description.is_none());
    }

    #[test]
    fn from_env_falls_back_to_defaults_when_unset() {
        let cfg = OpenApiConfig::from_env(&ConfigService::with_vars("openapi", [])).expect("ok");
        let d = OpenApiConfig::default();
        assert_eq!(cfg.enabled, d.enabled);
        assert_eq!(cfg.title, d.title);
        assert_eq!(cfg.version, d.version);
        assert!(cfg.description.is_none());
    }

    #[test]
    fn from_env_overrides_each_field_independently() {
        let service = ConfigService::with_vars(
            "openapi",
            [
                ("NESTRS_OPENAPI__ENABLED", "false"),
                ("NESTRS_OPENAPI__TITLE", "Custom API"),
                ("NESTRS_OPENAPI__VERSION", "9.9.9"),
                ("NESTRS_OPENAPI__DESCRIPTION", "Generated docs"),
            ],
        );
        let cfg = OpenApiConfig::from_env(&service).expect("ok");
        assert!(!cfg.enabled);
        assert_eq!(cfg.title, "Custom API");
        assert_eq!(cfg.version, "9.9.9");
        assert_eq!(cfg.description.as_deref(), Some("Generated docs"));
    }

    #[test]
    fn enabled_reads_boolean_spellings() {
        let off = ConfigService::with_vars("openapi", [("NESTRS_OPENAPI__ENABLED", "off")]);
        let cfg = OpenApiConfig::from_env(&off).expect("ok");
        assert!(!cfg.enabled, "`off` disables the documentation endpoints");

        let on = ConfigService::with_vars("openapi", [("NESTRS_OPENAPI__ENABLED", "true")]);
        let cfg = OpenApiConfig::from_env(&on).expect("ok");
        assert!(cfg.enabled);
    }

    // HTTP-S5: the public, unauthenticated docs default OFF outside a dev/test
    // profile — a deployed binary that forgets `NESTRS_OPENAPI__ENABLED` must not
    // publish its API surface.
    #[test]
    fn docs_default_off_outside_dev() {
        assert!(docs_default_enabled(Environment::Development));
        assert!(docs_default_enabled(Environment::Test));
        assert!(!docs_default_enabled(Environment::Staging));
        assert!(!docs_default_enabled(Environment::Production));
    }

    // The set-but-unparseable contract: a bad boolean must fail boot naming the
    // variable, never silently default the public docs back on.
    #[test]
    fn enabled_rejects_unparseable_value_naming_the_var() {
        let service = ConfigService::with_vars("openapi", [("NESTRS_OPENAPI__ENABLED", "maybe")]);
        let err = OpenApiConfig::from_env(&service)
            .expect_err("a non-boolean must fail, never silently default");
        assert!(
            matches!(err, nest_rs_config::ConfigError::Parse { ref var, .. } if var == "NESTRS_OPENAPI__ENABLED"),
            "the error must name the offending variable",
        );
    }
}
