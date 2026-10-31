use nest_rs_config::ConfigError;

/// The specification's list of the context formats a process propagates.
const OTEL_PROPAGATORS: &str = "OTEL_PROPAGATORS";

const ACCEPTED: &str = "must name tracecontext and baggage, what this process propagates whatever \
                        is set: W3C Trace Context at every edge, and W3C Baggage on the queue, \
                        which carries a job's baggage onto every push the job makes";

/// `OTEL_PROPAGATORS`, which may only say what the framework propagates: it
/// speaks W3C Trace Context at every edge itself, and the queue carries W3C
/// Baggage from a job to the pushes it makes, neither through a propagator
/// this variable picks. So a comma-separated list naming `tracecontext` and
/// `baggage`, in any order and case, is honoured, as unset is (the
/// specification's default is that list); anything else is refused naming the
/// variable, never its value — a format the framework does not speak (`b3`,
/// `jaeger`, `xray`), a list leaving one of the two out, and `none`, which
/// cannot turn off a correlation the framework owns.
pub(crate) fn read_propagators(read: &impl Fn(&str) -> Option<String>) -> Result<(), ConfigError> {
    let Some(raw) = read(OTEL_PROPAGATORS).filter(|raw| !raw.trim().is_empty()) else {
        return Ok(());
    };
    let (mut trace_context, mut baggage) = (false, false);
    for name in raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if name.eq_ignore_ascii_case("tracecontext") {
            trace_context = true;
        } else if name.eq_ignore_ascii_case("baggage") {
            baggage = true;
        } else if name.eq_ignore_ascii_case("none") {
            return Err(ConfigError::parse(
                OTEL_PROPAGATORS,
                "names none, which cannot turn propagation off: W3C Trace Context at every edge \
                 is the framework's correlation, which no variable switches off, and the queue \
                 carries a job's W3C Baggage onto every push the job makes",
            ));
        } else {
            return Err(ConfigError::parse(OTEL_PROPAGATORS, ACCEPTED));
        }
    }
    if trace_context && baggage {
        Ok(())
    } else {
        Err(ConfigError::parse(OTEL_PROPAGATORS, ACCEPTED))
    }
}

/// The SDK's global propagator, for what reads it outside the framework: the
/// two formats `OTEL_PROPAGATORS` may name, the specification's default.
#[cfg(feature = "otlp")]
pub(crate) fn global_propagator() -> opentelemetry::propagation::TextMapCompositePropagator {
    opentelemetry::propagation::TextMapCompositePropagator::new(vec![
        Box::new(opentelemetry_sdk::propagation::TraceContextPropagator::new()),
        Box::new(opentelemetry_sdk::propagation::BaggagePropagator::new()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::reading;

    /// Unset, blank, or both W3C formats in any order and case: what the
    /// framework propagates.
    #[test]
    fn the_two_w3c_formats_the_framework_propagates_are_honoured() {
        assert!(read_propagators(&|_: &str| None).is_ok());
        for value in [
            "",
            "  ",
            "tracecontext,baggage",
            "baggage, tracecontext",
            "TraceContext,BAGGAGE",
            "tracecontext,baggage,tracecontext",
        ] {
            assert!(
                read_propagators(&reading(&[(OTEL_PROPAGATORS, value)])).is_ok(),
                "{value:?}"
            );
        }
    }

    /// A format the framework does not speak, a list leaving one of the two
    /// out, or naming nothing, is refused naming the variable and what it
    /// accepts, never the value.
    #[test]
    fn a_format_the_framework_does_not_propagate_is_refused_naming_the_variable() {
        for value in [
            "b3",
            "b3multi",
            "jaeger",
            "xray",
            "ottrace",
            "tracecontext,baggage,b3",
            "tracecontext",
            "baggage",
            " , ",
        ] {
            let said = read_propagators(&reading(&[(OTEL_PROPAGATORS, value)]))
                .expect_err(value)
                .to_string();
            assert!(
                said.contains(OTEL_PROPAGATORS)
                    && said.contains("must name tracecontext and baggage"),
                "{said}"
            );
            let named_or_blank = matches!(value, "tracecontext" | "baggage")
                || !value.chars().any(char::is_alphanumeric);
            assert!(named_or_blank || !said.contains(value), "{said}");
        }
    }

    /// The SDK's global propagator injects and extracts both W3C formats, the
    /// specification's default list.
    #[cfg(feature = "otlp")]
    #[test]
    fn the_global_propagator_speaks_both_w3c_formats() {
        use opentelemetry::propagation::TextMapPropagator as _;

        let propagator = global_propagator();
        let fields: Vec<&str> = propagator.fields().collect();
        for field in ["traceparent", "tracestate", "baggage"] {
            assert!(fields.contains(&field), "{fields:?}");
        }
    }

    /// `none` cannot turn off a correlation the framework owns, and its
    /// refusal says so.
    #[test]
    fn none_is_refused_saying_the_framework_s_correlation_is_not_switched_off() {
        for value in ["none", "NONE", "tracecontext,none"] {
            let said = read_propagators(&reading(&[(OTEL_PROPAGATORS, value)]))
                .expect_err(value)
                .to_string();
            assert!(
                said.contains(OTEL_PROPAGATORS) && said.contains("cannot turn propagation off"),
                "{said}"
            );
        }
    }
}
