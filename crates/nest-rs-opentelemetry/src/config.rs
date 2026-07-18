use nest_rs_config::env_var;

/// Configuration for [`crate::OpenTelemetry::init`].
///
/// Env vars under the `NESTRS_OPENTELEMETRY__` prefix
/// (`NESTRS_OPENTELEMETRY__{LOG_LEVEL,LOG_FORMAT,LOG_SOURCE_LOCATION,SERVICE_NAME,
/// SERVICE_VERSION,SERVICE_ENVIRONMENT,SERVICE_INSTANCE_ID,OTLP_ENDPOINT,SAMPLE_RATIO}`).
/// OTel exporter is wired only when `otlp_endpoint` is set; otherwise the
/// subscriber stays console-only.
#[derive(Clone, Debug)]
pub struct OpenTelemetryConfig {
    /// `service.name` on every span, metric and log — the primary axis a
    /// backend groups telemetry by. Required (the one non-optional field);
    /// defaults to the value passed to [`new`](Self::new).
    pub service_name: String,
    /// `service.version` resource attribute (e.g. the crate version or git
    /// SHA). `None` omits the attribute entirely rather than emitting a blank.
    pub service_version: Option<String>,
    /// `deployment.environment` resource attribute (`prod`, `staging`, …) so a
    /// backend can partition otherwise-identical services. `None` omits it.
    pub deployment_environment: Option<String>,
    /// Defaults to a fresh UUID v7 per process so restarts get distinct
    /// identities in the backend.
    pub service_instance_id: Option<String>,
    /// `EnvFilter` syntax; applied to console layer and OTel log appender.
    pub log_filter: String,
    /// Console output shape: human-readable [`Text`](LogFormat::Text) in dev,
    /// machine-parseable [`Json`](LogFormat::Json) in prod. Defaults by build
    /// profile (see [`new`](Self::new)); the OTLP appender is unaffected.
    pub log_format: LogFormat,
    /// Append the emitting `file:line` to every console event. Useful in dev
    /// to locate a log's origin; off by default (adds width to every line and
    /// leaks source paths in prod).
    pub log_source_location: bool,
    /// Base endpoint (e.g. `http://localhost:4318`); exporter appends
    /// `/v1/traces`, `/v1/metrics`, `/v1/logs`.
    pub otlp_endpoint: Option<String>,
    /// `[0.0, 1.0]`; wrapped in `ParentBased` so children inherit the
    /// parent's sampling decision.
    pub trace_sample_ratio: f64,
}

/// Shape of the console log layer's output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable pretty-print for local development. The default in debug
    /// builds; pinning it is what keeps default apps readable at the terminal.
    #[default]
    Text,
    /// One JSON object per event for log aggregators. The default in release
    /// builds so a deployed app is machine-parseable without extra config.
    Json,
}

impl LogFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Canonical env-flag grammar shared by every `nest-rs-opentelemetry` boolean
/// var: `1`/`true`/`yes`/`on` → `true`, `0`/`false`/`no`/`off` → `false`,
/// anything else → `None`. Case-insensitive, trimmed. Callers apply their own
/// default for the unrecognized/absent case (source-location defaults off,
/// access-log defaults on), keeping the truthy/falsy vocabulary in one place.
pub(crate) fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl OpenTelemetryConfig {
    /// Config with framework defaults and the given `service.name`. `log_format`
    /// is chosen by build profile (Text in debug, Json in release); everything
    /// else is off/absent. The builder `with_*` methods and [`from_env`](Self::from_env)
    /// layer on top.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: None,
            deployment_environment: None,
            service_instance_id: None,
            log_filter: "info".into(),
            // Production output is OTLP/JSON; the human-readable pretty-print is
            // a dev affordance only. Default by build profile so a release deploy
            // that mounts `OpenTelemetryModule` emits JSON without needing
            // `NESTRS_OPENTELEMETRY__LOG_FORMAT` set (which still overrides).
            log_format: if cfg!(debug_assertions) {
                LogFormat::Text
            } else {
                LogFormat::Json
            },
            log_source_location: false,
            otlp_endpoint: None,
            trace_sample_ratio: 1.0,
        }
    }

    /// `service_name` is the default; `NESTRS_OPENTELEMETRY__SERVICE_NAME` overrides.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        let mut cfg = Self::new(service_name);

        if let Some(v) = env_var("NESTRS_OPENTELEMETRY__SERVICE_NAME") {
            cfg.service_name = v;
        }
        cfg.service_version = env_var("NESTRS_OPENTELEMETRY__SERVICE_VERSION");
        cfg.deployment_environment = env_var("NESTRS_OPENTELEMETRY__SERVICE_ENVIRONMENT");
        cfg.service_instance_id = env_var("NESTRS_OPENTELEMETRY__SERVICE_INSTANCE_ID");

        if let Some(v) = env_var("NESTRS_OPENTELEMETRY__LOG_LEVEL") {
            cfg.log_filter = v;
        }
        if let Some(raw) = env_var("NESTRS_OPENTELEMETRY__LOG_FORMAT")
            && let Some(fmt) = LogFormat::parse(&raw)
        {
            cfg.log_format = fmt;
        }
        if let Some(raw) = env_var("NESTRS_OPENTELEMETRY__LOG_SOURCE_LOCATION") {
            cfg.log_source_location = parse_bool(&raw).unwrap_or(false);
        }

        cfg.otlp_endpoint = env_var("NESTRS_OPENTELEMETRY__OTLP_ENDPOINT");
        if let Some(raw) = env_var("NESTRS_OPENTELEMETRY__SAMPLE_RATIO")
            && let Ok(r) = raw.parse::<f64>()
        {
            cfg.trace_sample_ratio = r.clamp(0.0, 1.0);
        }

        cfg
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

    /// Set the `deployment.environment` resource attribute.
    pub fn with_deployment_environment(mut self, env: impl Into<String>) -> Self {
        self.deployment_environment = Some(env.into());
        self
    }

    /// Set the trace sample ratio; the value is clamped into `[0.0, 1.0]` so a
    /// bad caller can't disable sampling logic outright.
    pub fn with_trace_sample_ratio(mut self, ratio: f64) -> Self {
        self.trace_sample_ratio = ratio.clamp(0.0, 1.0);
        self
    }
}

#[cfg(test)]
// The `figment::Jail::expect_with` closures below return `figment::Result`, so
// the large `Err` variant is figment's type, not ours — nothing to box here.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

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
        // Accepts `&str` and `String` — verify both compile and yield the same value.
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
        // Pin the default at the trait level — flipping to Json would change
        // every default app's log output overnight.
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }

    // `OpenTelemetryConfig::from_env` reads the `NESTRS_OPENTELEMETRY__*` keys
    // straight off the process env via `env_var`, so the tests isolate the env
    // with `figment::Jail` (the same approach `nest-rs-config` uses for its env
    // reads) — hermetic and serialized, no `unsafe { set_var }`. Vars a test
    // leaves unset are simply never `set_env`'d, exercising the default path.
    #[test]
    fn from_env_falls_back_to_defaults_for_every_optional_field() {
        figment::Jail::expect_with(|_| {
            let cfg = OpenTelemetryConfig::from_env("default-svc");
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

    #[test]
    fn from_env_overrides_each_field_when_set() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_OPENTELEMETRY__SERVICE_NAME", "override-svc");
            jail.set_env("NESTRS_OPENTELEMETRY__SERVICE_VERSION", "9.9.9");
            jail.set_env("NESTRS_OPENTELEMETRY__SERVICE_ENVIRONMENT", "prod");
            jail.set_env("NESTRS_OPENTELEMETRY__SERVICE_INSTANCE_ID", "pinned-1");
            jail.set_env("NESTRS_OPENTELEMETRY__LOG_LEVEL", "debug,hyper=warn");
            jail.set_env("NESTRS_OPENTELEMETRY__LOG_FORMAT", "json");
            jail.set_env("NESTRS_OPENTELEMETRY__LOG_SOURCE_LOCATION", "true");
            jail.set_env("NESTRS_OPENTELEMETRY__OTLP_ENDPOINT", "http://otel:4318");
            jail.set_env("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", "0.25");

            let cfg = OpenTelemetryConfig::from_env("default-svc");
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
    fn from_env_clamps_ratio_outside_zero_to_one() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", "2.5");
            assert_eq!(OpenTelemetryConfig::from_env("svc").trace_sample_ratio, 1.0);
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", "-0.5");
            assert_eq!(OpenTelemetryConfig::from_env("svc").trace_sample_ratio, 0.0);
            Ok(())
        });
    }

    #[test]
    fn from_env_ignores_unparseable_ratio_and_log_format() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", "not-a-number");
            jail.set_env("NESTRS_OPENTELEMETRY__LOG_FORMAT", "console");
            // Both stick to defaults — never panic on bad input.
            let cfg = OpenTelemetryConfig::from_env("svc");
            assert_eq!(cfg.trace_sample_ratio, 1.0);
            assert_eq!(cfg.log_format, LogFormat::Text);
            Ok(())
        });
    }
}
