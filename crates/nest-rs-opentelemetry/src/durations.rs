//! The duration histograms the framework records itself — a unit's, a client
//! operation's — each made once, under the framework's instrumentation scope,
//! into the bucket boundaries its declaration carries.

use nest_rs_core::duration_histogram::{DurationHistogram, MetricAttribute};
use opentelemetry::metrics::{Histogram, Meter, MeterProvider as _};
use opentelemetry::{InstrumentationScope, KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::SdkMeterProvider;

use crate::metrics::Made;

/// The instrumentation scope the framework's own instruments are recorded
/// under: the crate an app installs.
pub(crate) const SCOPE: &str = "nest-rs";

/// The unit a duration is in, as the conventions write it.
pub(crate) const SECONDS: &str = "s";

/// The framework's duration histograms, by name.
pub(crate) struct Durations {
    meter: Meter,
    histograms: Made<Histogram<f64>>,
}

impl Durations {
    /// Record on `provider`, under the framework's scope.
    pub(crate) fn new(provider: &SdkMeterProvider) -> Self {
        Self {
            meter: framework_meter(provider),
            histograms: Made::new(),
        }
    }

    /// Record `seconds` in the histogram `declared` names, keyed by
    /// `attributes`, making it the first time a duration records in it.
    pub(crate) fn record(
        &self,
        declared: &DurationHistogram,
        seconds: f64,
        attributes: &[KeyValue],
    ) {
        self.histograms.with(
            declared.name(),
            || {
                self.meter
                    .f64_histogram(declared.name())
                    .with_unit(SECONDS)
                    .with_description(declared.description())
                    .with_boundaries(declared.boundaries().to_vec())
                    .build()
            },
            |histogram| histogram.record(seconds, attributes),
        );
    }
}

/// The scope the framework's own instruments and spans are made under.
pub(crate) fn framework_scope() -> InstrumentationScope {
    InstrumentationScope::builder(SCOPE)
        .with_version(env!("CARGO_PKG_VERSION"))
        .build()
}

/// The meter the framework's own instruments are made on.
pub(crate) fn framework_meter(provider: &SdkMeterProvider) -> Meter {
    provider.meter_with_scope(framework_scope())
}

/// What the text `value`, held by a field keying a duration as `attribute`
/// says, keys it as: a member of the field's set as that member, never
/// copied, and `_OTHER` outside it; a bounded field's value as it is.
pub(crate) fn keyed_text(attribute: &MetricAttribute, value: &str) -> Value {
    Value::String(
        attribute
            .known_value(value)
            .map_or_else(|| StringValue::from(value.to_owned()), StringValue::from),
    )
}

/// What `value`, which no text carries, keys a duration as: as it is where
/// the field is bounded, through its set as written otherwise.
pub(crate) fn keyed_scalar(attribute: &MetricAttribute, value: Value) -> Value {
    if attribute.is_bounded() {
        value
    } else {
        keyed_text(attribute, &value.as_str())
    }
}
