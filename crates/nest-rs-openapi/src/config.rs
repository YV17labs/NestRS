//! [`OpenApiConfig`] — the OpenAPI document `info` block, populated from
//! `<PREFIX>_OPENAPI__*` in the `.env` cascade.

use std::path::PathBuf;

use nest_rs_config::{Config, ConfigError, ConfigService, Environment, Result, config};
use nest_rs_core::AppIdentity;

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
    /// Both endpoints are **public**, unauthenticated, so `Default` keeps them
    /// off — a config pinned over `..Default::default()` serves them in no
    /// profile — and only a development or test profile's
    /// [`Config::defaults`] turns them on. Enabling them outside such a
    /// profile is honoured and logged at `warn`.
    pub enabled: bool,
    /// The API title shown in the document `info` block and Swagger UI;
    /// defaults to the binary's package name ([`AppIdentity`]).
    pub title: String,
    /// The API version string in the `info` block — the app's version, not
    /// nestrs'; defaults to the binary's package version.
    pub version: String,
    /// A one-line summary of the API for the `info` block.
    pub summary: Option<String>,
    /// Long-form API description for the `info` block; defaults to the
    /// binary's package description, when its manifest has one.
    pub description: Option<String>,
    /// A URL to the API's terms of service.
    pub terms_of_service: Option<String>,
    /// Who answers for the API.
    pub contact: Option<OpenApiContact>,
    /// The license the API is offered under.
    pub license: Option<OpenApiLicense>,
    /// (Re)write [`emit_path`](Self::emit_path) with the document `/api-json`
    /// serves once at boot — the committed copy a pull request diffs. `Default`
    /// keeps it off, and only a development profile's [`Config::defaults`]
    /// turns it on: under test, parallel suites would rewrite a committed
    /// file. With [`enabled`](Self::enabled) off no document is built, so
    /// nothing is written and the boot says so at `warn`; a write failure is
    /// logged, never fatal.
    pub emit: bool,
    /// Where [`emit`](Self::emit) writes the JSON document. Default
    /// `openapi.json`; a relative path is the app crate's — the one
    /// `#[nest_rs::main]` expanded in — so each app of a workspace writes its
    /// own.
    pub emit_path: PathBuf,
}

/// `info.title` and `info.version` in a process no `#[nest_rs::main]` named.
const UNNAMED_TITLE: &str = "nestrs API";
const UNNAMED_VERSION: &str = "0.1.0";

impl Default for OpenApiConfig {
    /// The binary's name, version and description (its [`AppIdentity`]) in
    /// the `info` block, and every endpoint closed.
    fn default() -> Self {
        Self::named_by(AppIdentity::current())
    }
}

impl OpenApiConfig {
    /// The defaults, with the `info` block naming `app`, or a placeholder when
    /// no identity was captured.
    fn named_by(app: Option<&AppIdentity>) -> Self {
        Self {
            enabled: false,
            title: app.map_or(UNNAMED_TITLE, |app| app.name.as_str()).into(),
            version: app
                .map_or(UNNAMED_VERSION, |app| app.version.as_str())
                .into(),
            summary: None,
            description: app.and_then(|app| app.description.clone()),
            terms_of_service: None,
            contact: None,
            license: None,
            emit: false,
            emit_path: "openapi.json".into(),
        }
    }
}

impl Config for OpenApiConfig {
    // Not in `from_env`: overlaying it would rewrite a pinned value.
    fn defaults() -> Self {
        let environment = Environment::from_env();
        Self {
            enabled: !environment.is_deployed(),
            emit: environment == Environment::Development,
            ..Self::default()
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let d = base;
        let environment = Environment::from_env();
        let enabled = env.flag("ENABLED", d.enabled)?;
        if enabled && environment.is_deployed() {
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
            emit: env.flag("EMIT", d.emit)?,
            emit_path: env
                .get("EMIT_PATH")?
                .map(PathBuf::from)
                .unwrap_or(d.emit_path),
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

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::package_identity;

    use super::*;

    #[test]
    fn the_struct_default_serves_nothing_and_names_the_api() {
        let d = OpenApiConfig::default();
        assert!(
            !d.enabled,
            "public documentation is opened by the profile or in sight"
        );
        assert!(!d.title.is_empty());
        assert!(!d.version.is_empty());
        assert!(d.description.is_none());
    }

    #[test]
    fn the_info_defaults_to_the_binarys_identity() {
        let app = package_identity("api", "7.0.0", "The demo's API");
        let named = OpenApiConfig::named_by(Some(&app));
        assert_eq!(named.title, "api");
        assert_eq!(named.version, "7.0.0");
        assert_eq!(named.description.as_deref(), Some("The demo's API"));

        let undescribed = OpenApiConfig::named_by(Some(&package_identity("api", "7.0.0", "")));
        assert_eq!(undescribed.description, None);

        let unnamed = OpenApiConfig::named_by(None);
        assert!(!unnamed.title.is_empty() && !unnamed.version.is_empty());
        assert_eq!(unnamed.description, None);
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn a_pin_over_the_struct_default_keeps_the_docs_closed_in_production() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::Environment::var_name(), "production");
            let cfg = OpenApiConfig::from_env(
                &ConfigService::with_vars("openapi", []),
                OpenApiConfig {
                    title: "x".into(),
                    ..OpenApiConfig::default()
                },
            )
            .expect("ok");
            assert!(!cfg.enabled, "a pin opens the docs only by saying so");
            Ok(())
        });
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
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn the_unpinned_default_serves_the_documentation_in_development_and_test_alone() {
        figment::Jail::expect_with(|jail| {
            for (profile, enabled) in [
                ("development", true),
                ("test", true),
                ("staging", false),
                ("production", false),
                ("", false),
            ] {
                jail.set_env(nest_rs_config::Environment::var_name(), profile);
                assert_eq!(
                    <OpenApiConfig as Config>::defaults().enabled,
                    enabled,
                    "{profile:?}"
                );
            }
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn the_unpinned_default_emits_the_document_under_development_alone() {
        figment::Jail::expect_with(|jail| {
            for (profile, emits) in [
                ("development", true),
                ("test", false),
                ("staging", false),
                ("production", false),
                ("", false),
            ] {
                jail.set_env(nest_rs_config::Environment::var_name(), profile);
                assert_eq!(
                    <OpenApiConfig as Config>::defaults().emit,
                    emits,
                    "{profile:?}"
                );
            }
            Ok(())
        });
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
