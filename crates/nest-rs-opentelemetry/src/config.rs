use std::time::Duration;

use nest_rs_config::{
    Bound, ClientTls, ConfigError, ConfigService, DurationBounds, Floor, endpoint_shown,
};
use nest_rs_core::logging::LogSettings;

use crate::error::OpenTelemetryError;

/// The OTel SDK's own metric export period.
pub const DEFAULT_METRIC_INTERVAL: Duration = Duration::from_secs(60);

/// The namespace this crate's variables are read under — not a `#[config]`'s:
/// the config is read before the container exists.
pub(crate) const NAMESPACE: &str = "opentelemetry";

/// The metric export period's range, the variable that sets it, and why.
pub(crate) const METRIC_INTERVAL: DurationBounds = DurationBounds::secs(
    "METRIC_INTERVAL_SECS",
    "OpenTelemetryConfig::metric_interval",
    Floor::Units(Bound {
        count: 1,
        why: "a periodic reader on a shorter period exports in a tight loop, which costs the \
              collector more than any metric is worth",
    }),
    Bound {
        count: 60 * 60,
        why: "a metric exported less often than hourly reaches the collector after the \
              dashboards and alerts that read it have moved on, and the reader holds an hour of \
              state in memory meanwhile",
    },
);

/// Configuration for [`crate::OpenTelemetry::init`].
///
/// The console layer reads the framework-wide logging family (`<PREFIX>_LOG`,
/// `<PREFIX>_LOG_FORMAT`, `<PREFIX>_LOG_SOURCE_LOCATION`) through the kernel's
/// [`LogSettings`]; everything OTel-specific lives under
/// `<PREFIX>_OPENTELEMETRY__{SERVICE_NAME,SERVICE_VERSION,SERVICE_ENVIRONMENT,
/// SERVICE_INSTANCE_ID,OTLP_ENDPOINT,SAMPLE_RATIO,METRIC_INTERVAL_SECS,
/// TLS_CA_CERT,TLS_CERT,TLS_KEY}` (see [`EnvPrefix`](nest_rs_core::EnvPrefix)).
/// The OTel exporter is wired only when `otlp_endpoint` is set.
///
/// `Debug` shows the endpoint through [`endpoint_shown`]: its userinfo and
/// query may carry a collector's credential.
#[derive(Clone)]
pub struct OpenTelemetryConfig {
    /// `service.name` on every span, metric and log; defaults to the value passed
    /// to [`new`](Self::new).
    pub service_name: String,
    /// `service.version` resource attribute; `None` omits it.
    pub service_version: Option<String>,
    /// `deployment.environment.name` resource attribute; `None` omits it.
    pub deployment_environment: Option<String>,
    /// `service.instance.id`; defaults to a fresh UUID v7 per process.
    pub service_instance_id: Option<String>,
    /// `EnvFilter` syntax; applied to console layer and OTel log appender,
    /// narrowed by the clamps of the libraries the framework wires, and checked
    /// at [`init_with`](crate::OpenTelemetry::init_with).
    pub log_filter: String,
    /// Console output shape; defaults by build profile (see [`new`](Self::new)).
    /// The OTLP appender is unaffected.
    pub log_format: LogFormat,
    /// Append the emitting `file:line` to every console event; off by default,
    /// since it leaks source paths.
    pub log_source_location: bool,
    /// Base endpoint (e.g. `http://localhost:4318`); exporter appends
    /// `/v1/traces`, `/v1/metrics`, `/v1/logs`.
    pub otlp_endpoint: Option<String>,
    /// `[0.0, 1.0]`; wrapped in `ParentBased` so children inherit the
    /// parent's sampling decision.
    pub trace_sample_ratio: f64,
    /// How often the `PeriodicReader` flushes metrics to the collector; defaults to
    /// the OTel SDK's own 60 s.
    pub metric_interval: Duration,
    /// What an `https://` collector's certificate must chain to — the system's
    /// authorities unless `<PREFIX>_OPENTELEMETRY__TLS_CA_CERT` names others,
    /// which replace them — and the certificate presented to a collector
    /// requiring one (`TLS_CERT` and `TLS_KEY`). Refused beside an `http://`
    /// endpoint, which declares the export in plaintext.
    pub tls: ClientTls,
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

/// The OTLP endpoint, parsed: a value that does not parse is refused naming
/// `var`, the variable that sets it, and never quoting it.
pub(crate) fn otlp_endpoint(raw: &str, var: &str) -> Result<url::Url, ConfigError> {
    url::Url::parse(raw.trim())
        .map_err(|error| ConfigError::parse(var, format!("does not parse as a URL: {error}")))
}

/// The kernel's console format, whose grammar this crate's console shares.
pub use nest_rs_core::logging::LogFormat;

impl OpenTelemetryConfig {
    /// Config with framework defaults and the given `service.name`. `log_format`
    /// is chosen by build profile (Text in debug, Json in release); everything
    /// else is off/absent.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
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

    /// `service_name` is the default; `<PREFIX>_OPENTELEMETRY__SERVICE_NAME` overrides.
    ///
    /// `Err`, naming the variable and never quoting it, when one cannot be read
    /// or does not parse — the console's three through the kernel's
    /// [`LogSettings`], as its fallback subscriber reads them — or when
    /// `METRIC_INTERVAL_SECS` falls outside a second to an hour.
    pub fn from_env(service_name: impl Into<String>) -> Result<Self, OpenTelemetryError> {
        let mut cfg = Self::new(service_name);
        // Constructing the reader needs no container, which `from_env` runs before.
        let env = ConfigService::for_namespace(NAMESPACE);

        if let Some(v) = env.get("SERVICE_NAME")? {
            cfg.service_name = v;
        }
        cfg.service_version = env.get("SERVICE_VERSION")?;
        cfg.deployment_environment = env.get("SERVICE_ENVIRONMENT")?;
        cfg.service_instance_id = env.get("SERVICE_INSTANCE_ID")?;

        let console = LogSettings::from_env()?;
        cfg.log_filter = console.filter;
        cfg.log_format = console.format;
        cfg.log_source_location = console.source_location;

        cfg.otlp_endpoint = match env.setting("OTLP_ENDPOINT")? {
            Some(setting) => {
                otlp_endpoint(&setting.value, setting.var())?;
                Some(setting.value)
            }
            None => None,
        };
        if let Some(setting) = env.setting("SAMPLE_RATIO")? {
            let ratio = setting.parse::<f64>()?;
            // `NaN` parses as an `f64` and survives `clamp`.
            if ratio.is_nan() {
                return Err(setting.refuse("must be a number between 0 and 1").into());
            }
            cfg.trace_sample_ratio = ratio.clamp(0.0, 1.0);
        }
        cfg.metric_interval = METRIC_INTERVAL.read(&env, cfg.metric_interval)?.value;
        cfg.tls = ClientTls::from_env(&env, cfg.tls)?;

        Ok(cfg)
    }

    /// Pin the metric export interval, overriding the SDK's 60 s default. Held at
    /// [`OpenTelemetry::init_with`](crate::OpenTelemetry::init_with) to the range
    /// `<PREFIX>_OPENTELEMETRY__METRIC_INTERVAL_SECS` is: a second to an hour.
    pub fn with_metric_interval(mut self, interval: Duration) -> Self {
        self.metric_interval = interval;
        self
    }

    /// Override the `EnvFilter` directive string (e.g. `"debug,hyper=warn"`).
    pub fn with_log_filter(mut self, filter: impl Into<String>) -> Self {
        self.log_filter = filter.into();
        self
    }

    /// Pin the console [`LogFormat`], overriding the build-profile default.
    pub fn with_log_format(mut self, format: LogFormat) -> Self {
        self.log_format = format;
        self
    }

    /// Toggle appending `file:line` to each console event (off by default).
    pub fn with_log_source_location(mut self, enabled: bool) -> Self {
        self.log_source_location = enabled;
        self
    }

    /// Set the OTLP base endpoint, which is what enables the exporter — absent
    /// an endpoint the subscriber stays console-only.
    pub fn with_otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    /// Set the `service.version` resource attribute.
    pub fn with_service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Set the `deployment.environment.name` resource attribute.
    pub fn with_deployment_environment(mut self, env: impl Into<String>) -> Self {
        self.deployment_environment = Some(env.into());
        self
    }

    /// Set the trace sample ratio, clamped into `[0.0, 1.0]`.
    pub fn with_trace_sample_ratio(mut self, ratio: f64) -> Self {
        self.trace_sample_ratio = ratio.clamp(0.0, 1.0);
        self
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use super::*;
    use nest_rs_config::var_name;
    use nest_rs_core::EnvPrefix;
    use nest_rs_core::logging::var;

    #[test]
    fn defaults_sample_everything() {
        let cfg = OpenTelemetryConfig::new("svc");
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.log_filter, "info");
        assert_eq!(cfg.log_format, LogFormat::Text);
    }

    #[test]
    fn ratio_is_clamped() {
        let cfg = OpenTelemetryConfig::new("svc").with_trace_sample_ratio(2.5);
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        let cfg = OpenTelemetryConfig::new("svc").with_trace_sample_ratio(-1.0);
        assert_eq!(cfg.trace_sample_ratio, 0.0);
    }

    #[test]
    fn log_format_parses_canonical_names_only() {
        assert_eq!(LogFormat::parse("json"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("JSON"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("  text  "), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("console"), None);
        assert_eq!(LogFormat::parse("yaml"), None);
    }

    #[test]
    fn new_takes_the_service_name_as_owned_string() {
        let from_str = OpenTelemetryConfig::new("svc-a");
        let from_string = OpenTelemetryConfig::new(String::from("svc-a"));
        assert_eq!(from_str.service_name, from_string.service_name);
        assert_eq!(from_str.service_name, "svc-a");
    }

    #[test]
    fn defaults_have_no_optional_attrs() {
        let cfg = OpenTelemetryConfig::new("svc");
        assert!(cfg.service_version.is_none());
        assert!(cfg.deployment_environment.is_none());
        assert!(cfg.service_instance_id.is_none());
    }

    #[test]
    fn with_log_filter_overrides_the_default() {
        let cfg = OpenTelemetryConfig::new("svc").with_log_filter("debug,hyper=warn");
        assert_eq!(cfg.log_filter, "debug,hyper=warn");
    }

    #[test]
    fn with_log_format_pins_the_supplied_variant() {
        let cfg = OpenTelemetryConfig::new("svc").with_log_format(LogFormat::Json);
        assert_eq!(cfg.log_format, LogFormat::Json);
    }

    #[test]
    fn source_location_is_off_by_default() {
        assert!(!OpenTelemetryConfig::new("svc").log_source_location);
    }

    #[test]
    fn with_log_source_location_toggles_the_flag() {
        let cfg = OpenTelemetryConfig::new("svc").with_log_source_location(true);
        assert!(cfg.log_source_location);
    }

    #[test]
    fn with_otlp_endpoint_attaches_the_value() {
        let cfg = OpenTelemetryConfig::new("svc").with_otlp_endpoint("http://otel:4318");
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://otel:4318"));
    }

    #[test]
    fn with_service_version_attaches_the_value() {
        let cfg = OpenTelemetryConfig::new("svc").with_service_version("v1.2.3");
        assert_eq!(cfg.service_version.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn with_deployment_environment_attaches_the_value() {
        let cfg = OpenTelemetryConfig::new("svc").with_deployment_environment("prod");
        assert_eq!(cfg.deployment_environment.as_deref(), Some("prod"));
    }

    #[test]
    fn log_format_default_is_text() {
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }

    // `from_env` reads the process env, isolated per test by `figment::Jail`.
    #[test]
    fn from_env_falls_back_to_defaults_for_every_optional_field() {
        figment::Jail::expect_with(|_| {
            let cfg = OpenTelemetryConfig::from_env("default-svc").expect("readable config");
            assert_eq!(cfg.service_name, "default-svc");
            assert!(cfg.service_version.is_none());
            assert!(cfg.deployment_environment.is_none());
            assert!(cfg.service_instance_id.is_none());
            assert!(cfg.otlp_endpoint.is_none());
            assert_eq!(cfg.log_filter, "info");
            assert_eq!(cfg.log_format, LogFormat::Text);
            assert!(!cfg.log_source_location);
            assert_eq!(cfg.trace_sample_ratio, 1.0);
            Ok(())
        });
    }

    /// The endpoint may carry a collector's credential in its userinfo: a
    /// refusal names the variable, and `Debug` shows the endpoint without it.
    #[test]
    fn an_endpoint_is_refused_and_shown_without_its_value() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                var_name("opentelemetry", "OTLP_ENDPOINT"),
                "https://u:s3cr3t@[::1",
            );
            let refused = OpenTelemetryConfig::from_env("svc")
                .expect_err("an endpoint that does not parse")
                .to_string();
            assert!(
                refused.contains(&var_name("opentelemetry", "OTLP_ENDPOINT"))
                    && refused.contains("does not parse as a URL"),
                "{refused}"
            );
            assert!(!refused.contains("s3cr3t"), "{refused}");
            Ok(())
        });
        let config =
            OpenTelemetryConfig::new("svc").with_otlp_endpoint("https://u:s3cr3t@otel:4318");
        let debug = format!("{config:?}");
        assert!(debug.contains("https://otel:4318/"), "{debug}");
        assert!(!debug.contains("s3cr3t"), "{debug}");
    }

    #[test]
    fn from_env_overrides_each_field_when_set() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("opentelemetry", "SERVICE_NAME"), "override-svc");
            jail.set_env(var_name("opentelemetry", "SERVICE_VERSION"), "9.9.9");
            jail.set_env(var_name("opentelemetry", "SERVICE_ENVIRONMENT"), "prod");
            jail.set_env(var_name("opentelemetry", "SERVICE_INSTANCE_ID"), "pinned-1");
            jail.set_env(EnvPrefix::var(var::FILTER), "debug,hyper=warn");
            jail.set_env(EnvPrefix::var(var::FORMAT), "json");
            jail.set_env(EnvPrefix::var(var::SOURCE_LOCATION), "true");
            jail.set_env(
                var_name("opentelemetry", "OTLP_ENDPOINT"),
                "http://otel:4318",
            );
            jail.set_env(var_name("opentelemetry", "SAMPLE_RATIO"), "0.25");

            let cfg = OpenTelemetryConfig::from_env("default-svc").expect("readable config");
            assert_eq!(cfg.service_name, "override-svc");
            assert_eq!(cfg.service_version.as_deref(), Some("9.9.9"));
            assert_eq!(cfg.deployment_environment.as_deref(), Some("prod"));
            assert_eq!(cfg.service_instance_id.as_deref(), Some("pinned-1"));
            assert_eq!(cfg.log_filter, "debug,hyper=warn");
            assert_eq!(cfg.log_format, LogFormat::Json);
            assert!(cfg.log_source_location);
            assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://otel:4318"));
            assert!((cfg.trace_sample_ratio - 0.25).abs() < f64::EPSILON);
            Ok(())
        });
    }

    #[test]
    fn from_env_falls_back_to_rust_log_when_nestrs_log_is_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("RUST_LOG", "warn,tower=off");
            assert_eq!(
                OpenTelemetryConfig::from_env("svc")
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
                OpenTelemetryConfig::from_env("svc")
                    .expect("readable config")
                    .log_filter,
                "debug",
                "{} wins over RUST_LOG",
                EnvPrefix::var(var::FILTER),
            );
            Ok(())
        });
    }

    #[test]
    fn the_metric_interval_is_a_documented_default_an_env_var_and_a_builder() {
        assert_eq!(
            OpenTelemetryConfig::new("svc").metric_interval,
            DEFAULT_METRIC_INTERVAL,
            "the SDK's own period, named rather than implicit",
        );
        assert_eq!(DEFAULT_METRIC_INTERVAL, Duration::from_secs(60));
        assert_eq!(
            OpenTelemetryConfig::new("svc")
                .with_metric_interval(Duration::from_secs(5))
                .metric_interval,
            Duration::from_secs(5),
        );

        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("opentelemetry", "METRIC_INTERVAL_SECS"), "5");
            assert_eq!(
                OpenTelemetryConfig::from_env("svc")
                    .expect("readable config")
                    .metric_interval,
                Duration::from_secs(5),
            );
            Ok(())
        });
    }

    /// A `PeriodicReader` on a zero period is a tight export loop.
    #[test]
    fn an_empty_interval_keeps_the_default_and_a_zero_one_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("opentelemetry", "METRIC_INTERVAL_SECS"), "");
            assert_eq!(
                OpenTelemetryConfig::from_env("svc")
                    .expect("readable config")
                    .metric_interval,
                DEFAULT_METRIC_INTERVAL,
            );
            Ok(())
        });
        for (raw, sentence) in [
            ("0", "must be at least 1 second"),
            ("3601", "must be at most 3600 seconds"),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.set_env(var_name("opentelemetry", "METRIC_INTERVAL_SECS"), raw);
                let err = OpenTelemetryConfig::from_env("svc")
                    .expect_err("outside the range")
                    .to_string();
                assert!(
                    err.contains(&var_name("opentelemetry", "METRIC_INTERVAL_SECS"))
                        && err.contains(sentence),
                    "{err}"
                );
                Ok(())
            });
        }
        let pinned = METRIC_INTERVAL
            .check(
                NAMESPACE,
                "OpenTelemetryConfig::metric_interval",
                OpenTelemetryConfig::new("svc")
                    .with_metric_interval(Duration::ZERO)
                    .metric_interval,
            )
            .expect_err("a pinned zero is held to the floor where it is spent")
            .to_string();
        assert!(pinned.contains("set in code is 0ns"), "{pinned}");
    }

    #[test]
    fn from_env_clamps_ratio_outside_zero_to_one() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("opentelemetry", "SAMPLE_RATIO"), "2.5");
            assert_eq!(
                OpenTelemetryConfig::from_env("svc")
                    .expect("readable config")
                    .trace_sample_ratio,
                1.0
            );
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("opentelemetry", "SAMPLE_RATIO"), "-0.5");
            assert_eq!(
                OpenTelemetryConfig::from_env("svc")
                    .expect("readable config")
                    .trace_sample_ratio,
                0.0
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
                let refused = OpenTelemetryConfig::from_env("svc")
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
            let cfg = OpenTelemetryConfig::from_env("svc").expect("readable config");
            assert_eq!(cfg.log_format, LogFormat::by_profile());
            assert!(!cfg.log_source_location);
            Ok(())
        });
    }

    #[test]
    fn from_env_refuses_an_unparseable_ratio_or_interval_naming_it() {
        for key in ["SAMPLE_RATIO", "METRIC_INTERVAL_SECS"] {
            figment::Jail::expect_with(|jail| {
                jail.set_env(var_name("opentelemetry", key), "soon");
                let err = OpenTelemetryConfig::from_env("svc")
                    .unwrap_err()
                    .to_string();
                assert!(err.contains(&var_name("opentelemetry", key)), "{err}");
                Ok(())
            });
        }
    }

    #[test]
    fn from_env_refuses_a_ratio_that_is_not_a_number() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("opentelemetry", "SAMPLE_RATIO"), "NaN");
            let err = OpenTelemetryConfig::from_env("svc")
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(&var_name("opentelemetry", "SAMPLE_RATIO"))
                    && err.contains("between 0 and 1"),
                "{err}"
            );
            Ok(())
        });
    }
}
