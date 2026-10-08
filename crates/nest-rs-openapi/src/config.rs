//! [`OpenApiConfig`] — the OpenAPI document `info` block, populated from
//! `<PREFIX>_OPENAPI__*` in the `.env` cascade.

use std::path::PathBuf;

use nest_rs_config::{Config, ConfigError, ConfigService, Environment, Result, config};

use crate::contact::OpenApiContact;
use crate::license::OpenApiLicense;

/// The OpenAPI document's `info` block plus the master enable switch, settable
/// via `<PREFIX>_OPENAPI__*` or pinned through
/// [`OpenApiModule::for_root`](crate::OpenApiModule::for_root).
#[config(namespace = "openapi")]
#[derive(Clone, Debug)]
pub struct OpenApiConfig {
    /// Master switch for the documentation endpoints.
    ///
    /// Both endpoints are **public**, unauthenticated, so the unpinned default is
    /// off outside a dev/test profile; enabling them there is honoured and logged
    /// at `warn`.
    pub enabled: bool,
    /// The API title shown in the document `info` block and Swagger UI.
    pub title: String,
    /// The API version string in the `info` block (the app's version, not nestrs').
    pub version: String,
    /// A one-line summary of the API for the `info` block.
    pub summary: Option<String>,
    /// Optional long-form API description for the `info` block.
    pub description: Option<String>,
    /// A URL to the API's terms of service.
    pub terms_of_service: Option<String>,
    /// Who answers for the API.
    pub contact: Option<OpenApiContact>,
    /// The license the API is offered under.
    pub license: Option<OpenApiLicense>,
    /// (Re)write [`document_path`](Self::document_path) with the built document
    /// once at boot. Default `false`.
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
            summary: None,
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            emit_document: false,
            document_path: "openapi.json".into(),
        }
    }
}

impl Config for OpenApiConfig {
    // Not in `from_env`: overlaying it would rewrite a pinned `enabled: true`.
    fn defaults() -> Self {
        Self {
            enabled: docs_default_enabled(Environment::from_env()),
            ..Self::default()
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let d = base;
        let environment = Environment::from_env();
        let enabled = env.flag("ENABLED", d.enabled)?;
        if enabled && !docs_default_enabled(environment) {
            tracing::warn!(
                target: crate::TARGET,
                environment = environment.as_str(),
                "OpenAPI documentation endpoints are enabled and public outside a dev profile",
            );
        }
        Ok(Self {
            enabled,
            title: env.get("TITLE")?.unwrap_or(d.title),
            version: env.get("VERSION")?.unwrap_or(d.version),
            summary: env.get("SUMMARY")?.or(d.summary),
            description: env.get("DESCRIPTION")?.or(d.description),
            terms_of_service: env.get("TERMS_OF_SERVICE")?.or(d.terms_of_service),
            contact: contact_from_env(env, d.contact)?,
            license: license_from_env(env, d.license)?,
            emit_document: env.flag("EMIT_DOCUMENT", d.emit_document)?,
            document_path: env
                .get("DOCUMENT_PATH")?
                .map(PathBuf::from)
                .unwrap_or(d.document_path),
        })
    }
}

/// Each `CONTACT_*` key overlays its field; no field set is no contact.
fn contact_from_env(
    env: &ConfigService,
    base: Option<OpenApiContact>,
) -> Result<Option<OpenApiContact>> {
    let base = base.unwrap_or_default();
    let contact = OpenApiContact {
        name: env.get("CONTACT_NAME")?.or(base.name),
        url: env.get("CONTACT_URL")?.or(base.url),
        email: env.get("CONTACT_EMAIL")?.or(base.email),
    };
    Ok((contact != OpenApiContact::default()).then_some(contact))
}

/// `LICENSE_NAME` makes the license, which OpenAPI requires a name of; the
/// other keys overlay a license, never stand without one.
fn license_from_env(
    env: &ConfigService,
    base: Option<OpenApiLicense>,
) -> Result<Option<OpenApiLicense>> {
    let identifier = env.get("LICENSE_IDENTIFIER")?;
    let url = env.get("LICENSE_URL")?;
    let license = match (env.get("LICENSE_NAME")?, base) {
        (Some(name), base) => Some(OpenApiLicense {
            name,
            identifier: identifier.or(base.as_ref().and_then(|b| b.identifier.clone())),
            url: url.or(base.and_then(|b| b.url)),
        }),
        (None, Some(base)) => Some(OpenApiLicense {
            identifier: identifier.or(base.identifier),
            url: url.or(base.url),
            ..base
        }),
        (None, None) if identifier.is_some() || url.is_some() => {
            return Err(ConfigError::parse(
                env.var_name("LICENSE_NAME"),
                "is required beside LICENSE_IDENTIFIER or LICENSE_URL: OpenAPI's License \
                 Object names its license",
            ));
        }
        (None, None) => None,
    };
    if let Some(license) = &license
        && license.identifier.is_some()
        && license.url.is_some()
    {
        return Err(ConfigError::parse(
            env.var_name("LICENSE_URL"),
            "is exclusive of LICENSE_IDENTIFIER (OpenAPI 3.1 §4.8.4): name the license by \
             its SPDX identifier or by a URL to its text, not both",
        ));
    }
    Ok(license)
}

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
        let cfg =
            OpenApiConfig::from_env(&ConfigService::with_vars("openapi", []), Default::default())
                .expect("ok");
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
                ("ENABLED", "false"),
                ("TITLE", "Custom API"),
                ("VERSION", "9.9.9"),
                ("DESCRIPTION", "Generated docs"),
            ],
        );
        let cfg = OpenApiConfig::from_env(&service, Default::default()).expect("ok");
        assert!(!cfg.enabled);
        assert_eq!(cfg.title, "Custom API");
        assert_eq!(cfg.version, "9.9.9");
        assert_eq!(cfg.description.as_deref(), Some("Generated docs"));
    }

    #[test]
    fn the_contact_and_the_license_overlay_field_by_field() {
        let service = ConfigService::with_vars(
            "openapi",
            [
                ("CONTACT_EMAIL", "api@publish.example"),
                ("LICENSE_NAME", "Apache 2.0"),
                ("LICENSE_IDENTIFIER", "Apache-2.0"),
            ],
        );
        let base = OpenApiConfig {
            contact: Some(OpenApiContact {
                name: Some("API team".into()),
                ..OpenApiContact::default()
            }),
            ..OpenApiConfig::default()
        };
        let cfg = OpenApiConfig::from_env(&service, base).expect("ok");
        let contact = cfg.contact.expect("a contact");
        assert_eq!(contact.name.as_deref(), Some("API team"));
        assert_eq!(contact.email.as_deref(), Some("api@publish.example"));
        let license = cfg.license.expect("a license");
        assert_eq!(license.name, "Apache 2.0");
        assert_eq!(license.identifier.as_deref(), Some("Apache-2.0"));

        let none =
            OpenApiConfig::from_env(&ConfigService::with_vars("openapi", []), Default::default())
                .expect("ok");
        assert!(none.contact.is_none() && none.license.is_none());
    }

    #[test]
    fn a_license_named_by_identifier_and_url_at_once_is_refused() {
        let service = ConfigService::with_vars(
            "openapi",
            [
                ("LICENSE_NAME", "MIT"),
                ("LICENSE_IDENTIFIER", "MIT"),
                ("LICENSE_URL", "https://opensource.org/license/mit"),
            ],
        );
        let err = OpenApiConfig::from_env(&service, Default::default())
            .expect_err("OpenAPI makes them exclusive");
        assert!(
            matches!(err, nest_rs_config::ConfigError::Parse { ref var, .. } if *var == nest_rs_config::var_name("openapi", "LICENSE_URL")),
            "{err}",
        );
    }

    #[test]
    fn a_license_detail_without_its_name_is_refused() {
        let service = ConfigService::with_vars("openapi", [("LICENSE_IDENTIFIER", "MIT")]);
        let err = OpenApiConfig::from_env(&service, Default::default())
            .expect_err("OpenAPI requires the name");
        assert!(
            matches!(err, nest_rs_config::ConfigError::Parse { ref var, .. } if *var == nest_rs_config::var_name("openapi", "LICENSE_NAME")),
            "{err}",
        );
    }

    #[test]
    fn enabled_reads_boolean_spellings() {
        let off = ConfigService::with_vars("openapi", [("ENABLED", "off")]);
        let cfg = OpenApiConfig::from_env(&off, Default::default()).expect("ok");
        assert!(!cfg.enabled, "`off` disables the documentation endpoints");

        let on = ConfigService::with_vars("openapi", [("ENABLED", "true")]);
        let cfg = OpenApiConfig::from_env(&on, Default::default()).expect("ok");
        assert!(cfg.enabled);
    }

    #[test]
    fn docs_default_off_outside_dev() {
        assert!(docs_default_enabled(Environment::Development));
        assert!(docs_default_enabled(Environment::Test));
        assert!(!docs_default_enabled(Environment::Staging));
        assert!(!docs_default_enabled(Environment::Production));
    }

    #[test]
    fn enabled_rejects_unparseable_value_naming_the_var() {
        let service = ConfigService::with_vars("openapi", [("ENABLED", "maybe")]);
        let err = OpenApiConfig::from_env(&service, Default::default())
            .expect_err("a non-boolean must fail, never silently default");
        assert!(
            matches!(err, nest_rs_config::ConfigError::Parse { ref var, .. } if *var == nest_rs_config::var_name("openapi", "ENABLED")),
            "the error must name the offending variable",
        );
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn docs_enabled_outside_a_dev_profile_are_reported() {
        figment::Jail::expect_with(|jail| {
            let logs = nest_rs_testing::LogCapture::install();
            jail.set_env(nest_rs_config::Environment::var_name(), "production");

            let cfg = OpenApiConfig::from_env(
                &ConfigService::with_vars("openapi", [("ENABLED", "true")]),
                Default::default(),
            )
            .expect("an explicit `true` is honoured, not overridden");
            assert!(cfg.enabled, "the deployment's choice stands");

            let event = logs.expect_one(
                "nest_rs::openapi",
                "OpenAPI documentation endpoints are enabled and public outside a dev profile",
            );
            assert_eq!(event.level, "warn");
            assert_eq!(event.field("environment").as_deref(), Some("production"));
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn docs_enabled_in_development_are_silent() {
        figment::Jail::expect_with(|jail| {
            let logs = nest_rs_testing::LogCapture::install();
            jail.set_env(nest_rs_config::Environment::var_name(), "development");

            let _ = OpenApiConfig::from_env(
                &ConfigService::with_vars("openapi", [("ENABLED", "true")]),
                Default::default(),
            )
            .expect("ok");

            assert!(
                logs.find(
                    "nest_rs::openapi",
                    "OpenAPI documentation endpoints are enabled and public outside a dev profile",
                )
                .is_empty(),
                "the default posture is not an incident: {:#?}",
                logs.events(),
            );
            Ok(())
        });
    }
}
