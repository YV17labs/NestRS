//! [`UpDownCounter`] — the OpenTelemetry Metrics API's UpDownCounter.

use std::fmt;

use crate::Attribute;
use crate::attribute::with_key_values;

/// An instrument recording a sum that goes both ways — what is in flight: open
/// connections, queued jobs (the OpenTelemetry Metrics API's UpDownCounter).
///
/// Made by [`Metrics`](crate::Metrics), once per name; a cheap `Clone`, so a
/// service may keep one in a field.
#[derive(Clone)]
pub struct UpDownCounter {
    instrument: opentelemetry::metrics::UpDownCounter<i64>,
}

impl UpDownCounter {
    pub(crate) fn new(instrument: opentelemetry::metrics::UpDownCounter<i64>) -> Self {
        Self { instrument }
    }

    /// Adds `value`, negative or not, to the sum, under `attributes`.
    pub fn add(&self, value: i64, attributes: &[Attribute]) {
        with_key_values(attributes, |key_values| {
            self.instrument.add(value, key_values)
        });
    }
}

impl fmt::Debug for UpDownCounter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpDownCounter").finish_non_exhaustive()
    }
}
