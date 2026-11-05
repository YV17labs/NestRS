//! [`OpenApiConfig`] — the OpenAPI document `info` block, populated from
//! `<PREFIX>_OPENAPI__*` in the `.env` cascade.

use std::path::PathBuf;

use nest_rs_config::__private::{RetiredKey, RetiredKeys};
use nest_rs_config::{ConfigError, ConfigService, Field, Result, Surface, config};
use nest_rs_core::AppIdentity;

use crate::contact::OpenApiContact;
use crate::license::OpenApiLicense;

nest_rs_core::inventory::submit! {
    RetiredKeys::under("openapi", &[
        RetiredKey::renamed("EMIT_DOCUMENT", "EMIT"),
        RetiredKey::renamed("DOCUMENT_PATH", "EMIT_PATH"),
    ])
}

/// The OpenAPI document's `info` block plus the master enable switch, settable
/// via `<PREFIX>_OPENAPI__*` or pinned through
/// [`OpenApiModule::for_root`](crate::OpenApiModule::for_root).
#[config(namespace = "openapi")]
#[derive(Clone, Debug)]
pub struct OpenApiConfig {
    /// Master switch for the documentation endpoints.
    ///
    /// Both endpoints are **public**, unauthenticated, so `Default` leaves them
    /// to the profile: served under a declared development or test profile
    /// alone. Opening them outside one, [`Surface::Open`] or `true` from the
    /// environment, is honoured and logged at `warn`.
    pub enabled: Surface,
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
    #[config(with = contact)]
    pub contact: Option<OpenApiContact>,
    /// The license the API is offered under.
    #[config(with = license)]
    pub license: Option<OpenApiLicense>,
    /// (Re)write [`emit_path`](Self::emit_path) with the document `/api-json`
    /// serves once at boot — the committed copy a pull request diffs. `Default`
    /// leaves it to the profile: written under a declared development profile
    /// alone, since under test parallel suites would rewrite a committed file.
    /// With [`enabled`](Self::enabled) closed no document is built, so nothing
    /// is written and the boot says so at `warn`; writing it outside a
    /// development profile, [`Surface::Open`] or `true` from the environment, is
    /// honoured and logged at `warn`; a write failure is logged, never fatal.
    pub emit: Surface,
    /// Where [`emit`](Self::emit) writes the JSON document. Default
    /// `openapi.json`; a relative path is the app crate's — the one
    /// `#[nest_rs::main]` expanded in — so each app of a workspace writes its
    /// own.
    pub emit_path: PathBuf,
}

/// What the boot says when a deployment writes the committed document.
const EMIT_OPEN: &str = "OpenAPI document is written to disk outside a dev profile";

/// `info.title` and `info.version` in a process no `#[nest_rs::main]` named.
const UNNAMED_TITLE: &str = "nestrs API";
const UNNAMED_VERSION: &str = "0.1.0";

impl Default for OpenApiConfig {
    /// The binary's name, version and description (its [`AppIdentity`]) in
    /// the `info` block, and the endpoints left to the profile.
    fn default() -> Self {
        Self::named_by(AppIdentity::current())
    }
}

impl OpenApiConfig {
    /// The defaults, with the `info` block naming `app`, or a placeholder when
    /// no identity was captured.
    fn named_by(app: Option<&AppIdentity>) -> Self {
        Self {
            enabled: Surface::ByProfile,
            title: app.map_or(UNNAMED_TITLE, |app| app.name.as_str()).into(),
            version: app
                .map_or(UNNAMED_VERSION, |app| app.version.as_str())
                .into(),
            summary: None,
            description: app.and_then(|app| app.description.clone()),
            terms_of_service: None,
            contact: None,
            license: None,
            emit: Surface::ByProfile,
            emit_path: "openapi.json".into(),
        }
    }

    /// One `warn` when the documentation is served outside a development or
    /// test profile, and one when the document is written there: honoured, since
    /// the deployment chose it, and said. With the documentation closed nothing
    /// is written, which the boot says on its own.
    pub(crate) fn warn_when_open(&self) {
        let Some(environment) = self.enabled.opened_in_deployment() else {
            return;
        };
        tracing::warn!(
            target: crate::TARGET,
            environment = environment.as_str(),
            "OpenAPI documentation endpoints are enabled and public outside a dev profile",
        );
        if self.emit.is_open() {
            tracing::warn!(
                target: crate::TARGET,
                environment = environment.as_str(),
                "{EMIT_OPEN}",
            );
        }
    }
}

/// The contact, its vocabulary's `CONTACT_*` keys; none when every part is unset.
fn contact(
    env: &ConfigService,
    _: Field,
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

/// The license, its vocabulary's `LICENSE_*` keys: `LICENSE_NAME` makes it,
/// since OpenAPI requires a name, and the other keys overlay a license, never
/// stand without one; it is known by its SPDX identifier or its URL, never both.
fn license(
    env: &ConfigService,
    _: Field,
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
    use nest_rs_config::Config;
    use nest_rs_core::__private::package_identity;

    use super::*;

    #[test]
    fn the_struct_default_leaves_the_documentation_to_the_profile_and_names_the_api() {
        let d = OpenApiConfig::default();
        assert_eq!(d.enabled, Surface::ByProfile);
        assert_eq!(d.emit, Surface::ByProfile);
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
            assert!(
                !cfg.enabled.is_open(),
                "a deployment opens the docs only by saying so"
            );
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
        assert_eq!(cfg.enabled, Surface::Closed);
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
        assert_eq!(
            cfg.enabled,
            Surface::Closed,
            "`off` closes the documentation endpoints"
        );

        let on = ConfigService::with_vars("openapi", [("ENABLED", "true")]);
        let cfg = OpenApiConfig::from_env(&on, Default::default()).expect("ok");
        assert_eq!(cfg.enabled, Surface::Open);
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn the_default_serves_the_documentation_in_development_and_test_alone() {
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
                    OpenApiConfig::default().enabled.is_open(),
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
    fn the_default_emits_the_document_under_development_alone() {
        figment::Jail::expect_with(|jail| {
            for (profile, emits) in [
                ("development", true),
                ("test", false),
                ("staging", false),
                ("production", false),
                ("", false),
            ] {
                jail.set_env(nest_rs_config::Environment::var_name(), profile);
                assert_eq!(OpenApiConfig::default().emit.writes(), emits, "{profile:?}");
            }
            Ok(())
        });
    }

    /// A deployment writing the committed document is honoured, and says so:
    /// with the documentation closed nothing is written, and another line says that.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn an_emit_opened_outside_a_dev_profile_is_reported() {
        figment::Jail::expect_with(|jail| {
            for (profile, enabled, said) in [
                ("production", Surface::Open, true),
                ("staging", Surface::Open, true),
                ("", Surface::Open, true),
                ("production", Surface::Closed, false),
                ("development", Surface::Open, false),
                ("test", Surface::Open, false),
            ] {
                jail.set_env(nest_rs_config::Environment::var_name(), profile);
                let logs = nest_rs_testing::LogCapture::install();
                OpenApiConfig {
                    enabled,
                    emit: Surface::Open,
                    ..OpenApiConfig::default()
                }
                .warn_when_open();
                let line = logs.find(crate::TARGET, EMIT_OPEN);
                assert_eq!(line.len(), usize::from(said), "{profile:?} {enabled:?}");
                if said {
                    assert_eq!(line[0].level, "warn");
                }
            }
            jail.set_env(nest_rs_config::Environment::var_name(), "production");
            let logs = nest_rs_testing::LogCapture::install();
            OpenApiConfig::default().warn_when_open();
            assert!(
                logs.find(crate::TARGET, EMIT_OPEN).is_empty(),
                "the default writes nothing in a deployment"
            );
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
            assert!(cfg.enabled.is_open(), "the deployment's choice stands");
            cfg.warn_when_open();

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

            OpenApiConfig::from_env(
                &ConfigService::with_vars("openapi", [("ENABLED", "true")]),
                Default::default(),
            )
            .expect("ok")
            .warn_when_open();

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
