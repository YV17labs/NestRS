//! [`Histogram`] — the OpenTelemetry Metrics API's Histogram.

use std::fmt;

use crate::Attribute;
use crate::attribute::with_key_values;

/// An instrument recording a distribution — what is measured per event: a
/// latency, a payload's size (the OpenTelemetry Metrics API's Histogram).
///
/// Made by [`Metrics`](crate::Metrics), once per name; a cheap `Clone`, so a
/// service may keep one in a field.
#[derive(Clone)]
pub struct Histogram {
    instrument: opentelemetry::metrics::Histogram<f64>,
}

impl Histogram {
    pub(crate) fn new(instrument: opentelemetry::metrics::Histogram<f64>) -> Self {
        Self { instrument }
    }

    /// Records `value` into the distribution, under `attributes`.
    pub fn record(&self, value: f64, attributes: &[Attribute]) {
        with_key_values(attributes, |key_values| {
            self.instrument.record(value, key_values)
        });
    }
}

impl fmt::Debug for Histogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Histogram").finish_non_exhaustive()
    }
}
