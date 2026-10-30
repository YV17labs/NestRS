use nest_rs_config::ConfigError;

// Neither opentelemetry_sdk 0.33 nor opentelemetry-otlp 0.33 names the
// selectors: they are read under the specification's names.
const OTEL_TRACES_EXPORTER: &str = "OTEL_TRACES_EXPORTER";
const OTEL_METRICS_EXPORTER: &str = "OTEL_METRICS_EXPORTER";
const OTEL_LOGS_EXPORTER: &str = "OTEL_LOGS_EXPORTER";

/// One signal OpenTelemetry exports, whose selector every build reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Traces,
    Metrics,
    Logs,
}

/// What a signal's selector asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Unset: the specification's default, `otlp`, which exports once an
    /// endpoint is set and stays quiet without one.
    Unset,
    /// `otlp`, named: an endpoint must be set for it.
    Otlp,
    /// `none`: no exporter, whatever endpoint is set.
    None,
}

/// What a selector may name in this build, as its refusal says it.
#[cfg(feature = "otlp")]
const ACCEPTED: &str = "must name otlp, the one exporter this crate builds (OTLP over HTTP), or \
                        none, which turns the signal off";
#[cfg(not(feature = "otlp"))]
const ACCEPTED: &str = "must name none: this binary was built without the otlp feature, so it \
                        builds no exporter and none is all it can honour";

impl Signal {
    /// The specification's selector of its exporters.
    pub(crate) fn exporter_var(self) -> &'static str {
        match self {
            Self::Traces => OTEL_TRACES_EXPORTER,
            Self::Metrics => OTEL_METRICS_EXPORTER,
            Self::Logs => OTEL_LOGS_EXPORTER,
        }
    }

    /// Its selector, `OTEL_<SIGNAL>_EXPORTER`: `none`, or `otlp` where the
    /// binary builds the OTLP exporter, in any case, or a comma-separated list
    /// of them, as the specification allows; blank is unset. Anything else is
    /// refused naming the variable, never its value: an exporter the binary
    /// does not build would ship nothing where the deployment believes it
    /// ships.
    pub(crate) fn selection(
        self,
        read: &impl Fn(&str) -> Option<String>,
    ) -> Result<Selection, ConfigError> {
        let variable = self.exporter_var();
        let Some(raw) = read(variable).filter(|raw| !raw.trim().is_empty()) else {
            return Ok(Selection::Unset);
        };
        let (mut otlp, mut none) = (false, false);
        for name in raw
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if cfg!(feature = "otlp") && name.eq_ignore_ascii_case("otlp") {
                otlp = true;
            } else if name.eq_ignore_ascii_case("none") {
                none = true;
            } else {
                return Err(ConfigError::parse(variable, ACCEPTED));
            }
        }
        match (otlp, none) {
            (true, false) => Ok(Selection::Otlp),
            (false, true) => Ok(Selection::None),
            (true, true) => Err(ConfigError::parse(
                variable,
                "names none beside otlp — none turns the signal's exporters off, so it stands alone",
            )),
            (false, false) => Err(ConfigError::parse(variable, ACCEPTED)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNALS: [Signal; 3] = [Signal::Traces, Signal::Metrics, Signal::Logs];

    /// `variable` set to `value`, and nothing else.
    fn reading(variable: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |name: &str| (name == variable).then(|| value.to_owned())
    }

    /// `none`, in any case, alone or repeated, switches a signal off in every
    /// build, and a blank selector is unset.
    #[test]
    fn none_switches_a_signal_off_in_every_build_and_a_blank_selector_is_unset() {
        for signal in SIGNALS {
            let variable = signal.exporter_var();
            for value in ["none", " NONE ", "none,none"] {
                assert_eq!(
                    signal.selection(&reading(variable, value)).expect(value),
                    Selection::None
                );
            }
            for value in ["", "  "] {
                assert_eq!(
                    signal.selection(&reading(variable, value)).expect(value),
                    Selection::Unset
                );
            }
        }
    }

    /// With the `otlp` feature, `otlp` alone or in a list keeps the signal,
    /// and an exporter the crate does not build is refused naming what it
    /// accepts.
    #[cfg(feature = "otlp")]
    #[test]
    fn with_the_otlp_feature_otlp_is_kept_and_another_exporter_refused() {
        for signal in SIGNALS {
            let variable = signal.exporter_var();
            for value in ["otlp", "OTLP", "otlp, otlp"] {
                assert_eq!(
                    signal.selection(&reading(variable, value)).expect(value),
                    Selection::Otlp
                );
            }
            let said = signal
                .selection(&reading(variable, "zipkin"))
                .expect_err("an exporter the crate does not build")
                .to_string();
            assert!(
                said.contains(variable) && said.contains("must name otlp"),
                "{said}"
            );
            assert!(!said.contains("zipkin"), "{said}");
        }
    }

    /// Without the `otlp` feature the binary builds no exporter: every name but
    /// `none`, `otlp` included, is refused naming the variable and the feature,
    /// never the value.
    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_every_exporter_but_none_is_refused_naming_the_feature() {
        for signal in SIGNALS {
            let variable = signal.exporter_var();
            for value in [
                "otlp",
                "OTLP",
                "otlp,otlp",
                "none,otlp",
                "zipkin",
                "console",
                " , ",
            ] {
                let said = signal
                    .selection(&reading(variable, value))
                    .expect_err(value)
                    .to_string();
                assert!(
                    said.contains(variable) && said.contains("without the otlp feature"),
                    "{said}"
                );
                assert!(value == "otlp" || !said.contains(value), "{said}");
            }
        }
    }
}
