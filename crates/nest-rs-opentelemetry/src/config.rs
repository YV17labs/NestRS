use std::time::Duration;

use nest_rs_config::{ClientTls, endpoint_shown};
use nest_rs_core::logging::LogSettings;

use crate::error::OpenTelemetryError;

/// The OTel SDK's own metric export period.
pub const DEFAULT_METRIC_INTERVAL: Duration = Duration::from_secs(60);

/// What an app pins about its telemetry, under what the OpenTelemetry
/// specification's own variables (`OTEL_*`) set: those overlay it, field by
/// field, when [`OpenTelemetry::init_with`](crate::OpenTelemetry::init_with)
/// builds the SDK, which reads most of them itself.
///
/// A field is set one way, in a struct literal over
/// [`OpenTelemetryConfig::default`] or [`from_env`](Self::from_env):
///
/// ```
/// use nest_rs_opentelemetry::OpenTelemetryConfig;
///
/// let config = OpenTelemetryConfig {
///     trace_sample_ratio: 0.1,
///     ..OpenTelemetryConfig::default()
/// };
/// assert_eq!(config.service_name, None, "the binary's name");
/// ```
///
/// `Debug` shows the endpoint through [`endpoint_shown`]: its userinfo and
/// query may carry a collector's credential.
#[derive(Clone)]
pub struct OpenTelemetryConfig {
    /// `service.name` on every span, metric and log; `None` is the binary's
    /// package name ([`AppIdentity`](nest_rs_core::AppIdentity)).
    /// `OTEL_SERVICE_NAME`, then `service.name` in `OTEL_RESOURCE_ATTRIBUTES`,
    /// override it.
    pub service_name: Option<String>,
    /// `service.version`; `None` is the binary's package version.
    /// `service.version` in `OTEL_RESOURCE_ATTRIBUTES` overrides it.
    pub service_version: Option<String>,
    /// `deployment.environment.name`; `None` omits it.
    /// `OTEL_RESOURCE_ATTRIBUTES` overrides it.
    pub deployment_environment: Option<String>,
    /// `service.instance.id`; `None` is a fresh UUID v7 per process.
    /// `OTEL_RESOURCE_ATTRIBUTES` overrides it.
    pub service_instance_id: Option<String>,
    /// `EnvFilter` syntax; applied to console layer and OTel log appender,
    /// narrowed by the clamps of the libraries the framework wires, and checked
    /// at [`init_with`](crate::OpenTelemetry::init_with).
    pub log_filter: String,
    /// Console output shape; defaults by the declared profile
    /// ([`LogFormat::by_profile`]). The OTLP appender is unaffected.
    pub log_format: LogFormat,
    /// Append the emitting `file:line` to every console event; off by default,
    /// since it leaks source paths.
    pub log_source_location: bool,
    /// Base endpoint (e.g. `http://localhost:4318`), to which each signal
    /// appends `/v1/traces`, `/v1/metrics` or `/v1/logs`; `None` exports
    /// nothing. `OTEL_EXPORTER_OTLP_ENDPOINT`, and per signal
    /// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` (`_METRICS_`, `_LOGS_`) used as
    /// given, override it.
    pub otlp_endpoint: Option<String>,
    /// `[0.0, 1.0]`, wrapped in `ParentBased` so children inherit the parent's
    /// sampling decision; `OTEL_TRACES_SAMPLER` (with `OTEL_TRACES_SAMPLER_ARG`)
    /// replaces the sampler whole.
    pub trace_sample_ratio: f64,
    /// How often the `PeriodicReader` flushes metrics to the collector, a second
    /// to an hour; defaults to the OTel SDK's own 60 s.
    /// `OTEL_METRIC_EXPORT_INTERVAL`, in milliseconds, overrides it.
    pub metric_interval: Duration,
    /// What an `https://` collector's certificate must chain to — the system's
    /// authorities unless others are named, which replace them — and the
    /// certificate presented to a collector requiring one. The files
    /// `OTEL_EXPORTER_OTLP_CERTIFICATE`, `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE`
    /// and `OTEL_EXPORTER_OTLP_CLIENT_KEY` name, and their per-signal
    /// spellings, override it. Refused beside an `http://` endpoint, which
    /// declares the export in plaintext.
    pub tls: ClientTls,
}

impl Default for OpenTelemetryConfig {
    /// The service named after the binary, the console's settings at their
    /// defaults (`log_format` by the declared profile), every trace sampled,
    /// the SDK's metric period, and no export.
    fn default() -> Self {
        Self {
            service_name: None,
            service_version: None,
            deployment_environment: None,
            service_instance_id: None,
            log_filter: "info".into(),
            log_format: LogFormat::by_profile(),
            log_source_location: false,
            otlp_endpoint: None,
            trace_sample_ratio: 1.0,
            metric_interval: DEFAULT_METRIC_INTERVAL,
            tls: ClientTls::default(),
        }
    }
}

impl std::fmt::Debug for OpenTelemetryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenTelemetryConfig")
            .field("service_name", &self.service_name)
            .field("service_version", &self.service_version)
            .field("deployment_environment", &self.deployment_environment)
            .field("service_instance_id", &self.service_instance_id)
            .field("log_filter", &self.log_filter)
            .field("log_format", &self.log_format)
            .field("log_source_location", &self.log_source_location)
            .field(
                "otlp_endpoint",
                &self.otlp_endpoint.as_deref().map(endpoint_shown),
            )
            .field("trace_sample_ratio", &self.trace_sample_ratio)
            .field("metric_interval", &self.metric_interval)
            .field("tls", &self.tls)
            .finish()
    }
}

/// The kernel's console format, whose grammar this crate's console shares.
pub use nest_rs_core::logging::LogFormat;

impl OpenTelemetryConfig {
    /// The defaults, with the console's three settings read from the
    /// framework-wide logging family (`<PREFIX>_LOG`, `<PREFIX>_LOG_FORMAT`,
    /// `<PREFIX>_LOG_SOURCE_LOCATION`) by the kernel's [`LogSettings`], as its
    /// fallback subscriber reads them. Everything OpenTelemetry's own is read
    /// at [`init_with`](crate::OpenTelemetry::init_with), from `OTEL_*`.
    ///
    /// `Err`, naming the variable and never quoting it, when one cannot be read
    /// or does not parse.
    pub fn from_env() -> Result<Self, OpenTelemetryError> {
        let console = LogSettings::from_env()?;
        Ok(Self {
            log_filter: console.filter,
            log_format: console.format,
            log_source_location: console.source_location,
            ..Self::default()
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use super::*;
    use nest_rs_core::EnvPrefix;
    use nest_rs_core::logging::var;

    #[test]
    fn the_default_names_nothing_samples_everything_and_exports_nothing() {
        let cfg = OpenTelemetryConfig::default();
        assert_eq!(cfg.service_name, None, "the binary's, resolved at init");
        assert_eq!(cfg.service_version, None);
        assert!(cfg.deployment_environment.is_none() && cfg.service_instance_id.is_none());
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.log_filter, "info");
        assert_eq!(cfg.log_format, LogFormat::by_profile());
        assert!(!cfg.log_source_location, "it leaks source paths");
        assert_eq!(cfg.metric_interval, DEFAULT_METRIC_INTERVAL);
        assert_eq!(DEFAULT_METRIC_INTERVAL, Duration::from_secs(60));
    }

    #[test]
    fn log_format_parses_canonical_names_only() {
        assert_eq!(LogFormat::parse("json"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("JSON"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("  text  "), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("console"), None);
        assert_eq!(LogFormat::parse("yaml"), None);
    }

    /// The endpoint may carry a collector's credential in its userinfo.
    #[test]
    fn debug_shows_the_endpoint_without_its_credential() {
        let config = OpenTelemetryConfig {
            otlp_endpoint: Some("https://u:s3cr3t@otel:4318".into()),
            ..OpenTelemetryConfig::default()
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("https://otel:4318/"), "{debug}");
        assert!(!debug.contains("s3cr3t"), "{debug}");
    }

    /// `from_env` reads the console's three and nothing else.
    #[test]
    fn from_env_reads_the_console_settings_over_the_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(EnvPrefix::var(var::FILTER), "debug,hyper=warn");
            jail.set_env(EnvPrefix::var(var::FORMAT), "json");
            jail.set_env(EnvPrefix::var(var::SOURCE_LOCATION), "true");

            let cfg = OpenTelemetryConfig::from_env().expect("readable config");
            assert_eq!(cfg.log_filter, "debug,hyper=warn");
            assert_eq!(cfg.log_format, LogFormat::Json);
            assert!(cfg.log_source_location);
            assert_eq!(cfg.service_name, None);
            assert!(cfg.otlp_endpoint.is_none());
            Ok(())
        });
    }

    #[test]
    fn from_env_falls_back_to_rust_log_when_nestrs_log_is_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("RUST_LOG", "warn,tower=off");
            assert_eq!(
                OpenTelemetryConfig::from_env()
                    .expect("readable config")
                    .log_filter,
                "warn,tower=off"
            );
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env(EnvPrefix::var(var::FILTER), "debug");
            jail.set_env("RUST_LOG", "warn");
            assert_eq!(
                OpenTelemetryConfig::from_env()
                    .expect("readable config")
                    .log_filter,
                "debug",
                "{} wins over RUST_LOG",
                EnvPrefix::var(var::FILTER),
            );
            Ok(())
        });
    }

    /// The console settings are the kernel's, read by its one parser: a value
    /// that does not parse is refused by name, as the fallback subscriber
    /// refuses it, and never quoted.
    #[test]
    fn from_env_refuses_a_log_setting_that_does_not_parse_naming_it_and_not_its_value() {
        for (tail, value) in [
            (var::FORMAT, "console"),
            (var::SOURCE_LOCATION, "ture"),
            (var::FILTER, "billing=notalevel"),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.set_env(EnvPrefix::var(tail), value);
                let refused = OpenTelemetryConfig::from_env()
                    .expect_err("a setting that does not parse")
                    .to_string();
                assert!(refused.contains(&EnvPrefix::var(tail)), "{refused}");
                assert!(!refused.contains(value), "{refused}");
                Ok(())
            });
        }
    }

    #[test]
    fn a_blank_log_setting_is_its_default() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(EnvPrefix::var(var::FORMAT), " ");
            jail.set_env(EnvPrefix::var(var::SOURCE_LOCATION), "");
            let cfg = OpenTelemetryConfig::from_env().expect("readable config");
            assert_eq!(cfg.log_format, LogFormat::by_profile());
            assert!(!cfg.log_source_location);
            Ok(())
        });
    }
}
