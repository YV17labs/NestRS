//! [`Counter`] — the OpenTelemetry Metrics API's Counter.

use std::fmt;

use crate::Attribute;
use crate::attribute::with_key_values;

/// An instrument recording a monotonic sum — what only grows: requests served, jobs failed (the OpenTelemetry Metrics API's Counter).
///
/// Made by [`Metrics`](crate::Metrics), once per name; a cheap `Clone`, so a
/// service may keep one in a field.
#[derive(Clone)]
pub struct Counter {
    instrument: opentelemetry::metrics::Counter<u64>,
}

impl Counter {
    pub(crate) fn new(instrument: opentelemetry::metrics::Counter<u64>) -> Self {
        Self { instrument }
    }

    /// Adds `value` to the sum, under `attributes`.
    pub fn add(&self, value: u64, attributes: &[Attribute]) {
        with_key_values(attributes, |key_values| {
            self.instrument.add(value, key_values)
        });
    }
}

impl fmt::Debug for Counter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Counter").finish_non_exhaustive()
    }
}
