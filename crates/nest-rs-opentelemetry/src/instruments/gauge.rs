//! [`Gauge`] — the OpenTelemetry Metrics API's Gauge.

use std::fmt;

use crate::Attribute;
use crate::attribute::with_key_values;

/// An instrument recording the last value measured — what is sampled, not
/// summed: a pool's idle connections, a temperature (the OpenTelemetry Metrics
/// API's Gauge).
///
/// Made by [`Metrics`](crate::Metrics), once per name; a cheap `Clone`, so a
/// service may keep one in a field.
#[derive(Clone)]
pub struct Gauge {
    instrument: opentelemetry::metrics::Gauge<f64>,
}

impl Gauge {
    pub(crate) fn new(instrument: opentelemetry::metrics::Gauge<f64>) -> Self {
        Self { instrument }
    }

    /// Records `value` as the current one, under `attributes`.
    pub fn record(&self, value: f64, attributes: &[Attribute]) {
        with_key_values(attributes, |key_values| {
            self.instrument.record(value, key_values)
        });
    }
}

impl fmt::Debug for Gauge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gauge").finish_non_exhaustive()
    }
}
